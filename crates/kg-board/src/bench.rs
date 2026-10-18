//! The `serve` testbench, generated from a board's resources: the board's
//! core module with each resource the design uses on the host
//! (`docs/plans/board-tooling.md` §9). Any board with `[resources.*]` gets
//! it; a board naming its own `bridge` in `board.toml` gets that instead.
//!
//! The clock is a port, toggled by `platform/sim/main.cpp` as fast as
//! Verilator evaluates, so the testbench builds without `--timing` and runs
//! three times faster than one clocking itself.
//!
//! Each resource the design uses becomes a widget on the front panel
//! (`platform/sim/vboard.cpp`, served by the simulation itself); the rest
//! idle, and the page lists them as unused:
//! - `led`: shown live, and printed as it changes;
//! - `button`: pressed on the page, read every 1,024 cycles;
//! - `uart`: a pseudo-terminal (`BOARD_PTY`, `platform/sim/uart_pty.cpp`)
//!   and the page's console, both typing into the design; under the host
//!   bridge, the page shows its calls instead;
//! - `spi`, `i2c`: a device model on the bus (`platform/sim/models`), its
//!   transactions counted;
//! - `vga`: the screen, read off the pins (`platform/sim/vga_capture.sv`).

use std::fmt::Write as _;

use camino::Utf8PathBuf;
use kg_config::Board;
use serde_json::{Value, json};

use crate::interface::Interface;
use crate::wrapper::{self, Core, Pin, Target, VGA_H, VGA_V};
use crate::{Design, Failure, Result};

/// What the design does with the board's resources, as its generated core
/// binds them: the panel shows what the design uses and nothing else, so a
/// design that lights LEDs and draws on the screen is not offered switches
/// it never reads. A core of the project's own says nothing of the kind, so
/// then every resource is put on the host.
#[derive(Debug, Clone, Copy, Default)]
pub struct Usage<'a> {
    pub core: Option<&'a Core>,
    pub interface: Option<&'a Interface>,
}

impl Usage<'_> {
    pub fn of<'a>(design: &'a Design) -> Usage<'a> {
        Usage { core: design.core.as_ref(), interface: design.interface.as_ref().ok() }
    }

    /// Whether a function binds to `resource`, or the bridge runs over it.
    pub fn uses(&self, resource: &str) -> bool {
        match self.core {
            None => true,
            Some(_) => self.bridged(resource) || !self.served(resource).is_empty(),
        }
    }

    /// Whether the host bridge runs over `resource`.
    pub fn bridged(&self, resource: &str) -> bool {
        self.core.and_then(|c| c.bridge.as_ref()).is_some_and(|p| p.resource == resource)
    }

    /// The functions serving `resource`, each with its role.
    fn served(&self, resource: &str) -> Vec<(&str, &str)> {
        let Some(core) = self.core else { return Vec::new() };
        core.bindings
            .iter()
            .filter_map(|(f, t)| match t {
                Target::Resource { resource: r, role } if r == resource => Some((f.as_str(), role.as_str())),
                _ => None,
            })
            .collect()
    }
}

/// The generated testbench and what it puts on the host.
#[derive(Debug)]
pub struct Bench {
    /// `tb.sv`, then the sources it needs beside it ([`support`]).
    pub files: Vec<Utf8PathBuf>,
    /// The panel's description of the design ([`describe`]).
    pub description: Utf8PathBuf,
    /// A serial port reaches the host, so the pseudo-terminal opens.
    pub pty: bool,
}

/// The sources the generated testbench needs beside itself, under the
/// platform: the panel's and the pseudo-terminal's C++, the VGA capture and
/// the device models of the resources the design uses.
pub fn support(board: &Board, usage: &Usage) -> Vec<Utf8PathBuf> {
    let sim = board.platform.join("sim");
    let mut files = vec![sim.join("main.cpp"), sim.join("vboard.cpp"), sim.join("uart_pty.cpp")];
    let has = |kind: &str| board.spec.resources.iter().any(|(n, r)| r.kind == kind && usage.uses(n));
    if has("vga") {
        files.push(sim.join("vga_capture.sv"));
    }
    if has("spi") {
        files.push(sim.join("models/spi_flash_model.sv"));
    }
    if has("i2c") {
        files.push(sim.join("models/i2c_target_model.sv"));
    }
    files
}

/// Writes the testbench (top `tb`) and the panel's description of the
/// design to `<out>/bench`.
pub fn write(design: &Design) -> Result<Bench> {
    let board = design.board;
    let usage = Usage::of(design);
    let dir = design.out.join("bench");
    std::fs::create_dir_all(&dir).map_err(|e| Failure::Failed(format!("{dir}: {e}")))?;
    let path = dir.join("tb.sv");
    crate::write(&path, &generate(board, &usage)?)?;
    let description = dir.join("panel.json");
    crate::write(&description, &(serde_json::to_string_pretty(&describe(board, &usage)).expect("JSON serialises") + "\n"))?;
    let mut files = vec![path];
    files.extend(support(board, &usage));
    Ok(Bench { files, description, pty: console(board, &usage).is_some() })
}

/// The serial port the host reaches: the first the design uses.
fn console<'b>(board: &'b Board, usage: &Usage) -> Option<&'b str> {
    board.spec.resources.iter().find(|(n, r)| r.kind == "uart" && usage.uses(n)).map(|(n, _)| n.as_str())
}

/// What the page says of the design beyond the widgets the testbench
/// declares: each resource's serving functions and their documentation,
/// the bridge's functions (the page decodes its traffic by them), and the
/// resources the design leaves idle. Served at `/layout` as `design`.
pub fn describe(board: &Board, usage: &Usage) -> Value {
    let function = |name: &str| usage.interface.and_then(|i| i.function(name));
    let doc = |name: &str| function(name).and_then(|f| f.doc.clone());
    let mut resources = serde_json::Map::new();
    let mut unused = Vec::new();
    for (name, r) in &board.spec.resources {
        if !usage.uses(name) {
            unused.push(json!({ "name": name, "kind": r.kind, "doc": r.doc }));
            continue;
        }
        let serves: Vec<Value> = usage
            .served(name)
            .into_iter()
            .map(|(f, role)| {
                json!({ "function": f, "role": role, "doc": doc(f), "signature": function(f).map(|f| f.signature()) })
            })
            .collect();
        resources.insert(name.clone(), json!({ "kind": r.kind, "doc": r.doc, "baud": r.baud, "hz": r.hz, "serves": serves }));
    }
    let bridge = usage.core.and_then(|c| c.bridge.as_ref()).map(|plan| {
        let functions: Vec<Value> = plan
            .functions
            .iter()
            .filter_map(|name| function(name))
            .map(|f| json!({ "name": f.name, "callback": f.callback, "index": f.index, "signature": f.signature(), "doc": f.doc }))
            .collect();
        let class = usage.interface.map(|i| i.module.clone()).unwrap_or_default();
        json!({
            "resource": plan.resource,
            "baud": plan.baud,
            "hash": format!("{:08x}", plan.hash),
            "python": crate::python::snake(&class),
            "class": class,
            "functions": functions,
        })
    });
    json!({
        "design": usage.interface.map(|i| i.module.clone()),
        "core": if usage.core.is_some() { "generated" } else { "own" },
        "resources": resources,
        "unused": unused,
        "bridge": bridge,
    })
}

pub fn generate(board: &Board, usage: &Usage) -> Result<String> {
    let core = board
        .core_module()
        .ok_or_else(|| Failure::Usage(format!("the board {} names no core-module for `serve`", board.name())))?;
    let mhz = board
        .spec
        .clock
        .mhz
        .ok_or_else(|| Failure::Usage(format!("the board {} gives no [clock] mhz, which `serve` needs", board.name())))?;
    let pins = wrapper::resource_pins(board)?;
    if pins.is_empty() {
        return Err(Failure::Usage(format!(
            "the board {} has no [resources.*] for `serve` to put on the host, and names no bridge of its own",
            board.name()
        )));
    }
    let resources = &board.spec.resources;
    let uart = console(board, usage);
    let used = |name: &str| usage.uses(name);

    let mut t = String::new();
    let _ = writeln!(t, "// Generated by `kg board` from the {} board's [resources]: its core with", board.name());
    let _ = writeln!(t, "// each resource the design uses on the host (crates/kg-board/src/bench.rs).");
    let _ = writeln!(t, "// `kg board serve` runs it until killed; the front panel's address is printed.\n");
    let _ = writeln!(t, "`default_nettype none\n\n// The clock is platform/sim/main.cpp's, at the board's {mhz} MHz as far as the\n// design can tell: it counts cycles, never time.\nmodule tb (input logic clk);");
    for import in [
        "function int uart_pty_open(input string path_file)",
        "function int uart_pty_getc()",
        "function void uart_pty_putc(input int c)",
        "function int vb_open(input string page, input string port_file, input string description, input string board, input int mhz, input int port, input int restart)",
        "function int vb_declare(input string kind, input string name, input int width, input string style, input string doc)",
        "function void vb_output(input int id, input int value)",
        "function int vb_input(input int id)",
        "function void vb_uart_out(input int c)",
        "function int vb_uart_in()",
        "function void vb_uart_sent(input int c)",
        "function int vb_tick(input int n)",
    ] {
        let _ = writeln!(t, "    import \"DPI-C\" {import};");
    }
    // Reset for the first 16 cycles, and again when the page asks; the
    // panel counts cycles (its speed readout), and holds the clock here
    // while paused, every 1,024.
    let _ = writeln!(
        t,
        "\n    logic [4:0] por = '0;\n    wire rst = por != 5'd16;\n    logic [9:0] tick = '0;\n    always @(posedge clk) begin\n        tick <= tick + 10'd1;\n        if (rst) por <= por + 5'd1;\n        if (tick == '0 && vb_tick(1024) != 0) por <= '0;\n    end\n"
    );

    // A signal per core port, named as the port.
    let mut ids = String::new();
    for (name, list) in &pins {
        let r = &resources[name];
        for pin in list {
            let range = if pin.width > 1 { format!("[{}:0] ", pin.width - 1) } else { String::new() };
            let kind = if pin.input && r.kind == "button" && used(name) { "logic" } else { "wire " };
            let _ = writeln!(t, "    {kind} {range}{};", pin.port);
        }
        if used(name) {
            let _ = writeln!(ids, "    int {name}_id;");
        }
    }
    t.push_str(&ids);

    let _ = writeln!(t, "\n    {core} dut\n    (\n        .clk(clk),\n        .rst(rst),");
    let all: Vec<&Pin> = pins.iter().flat_map(|(_, l)| l).collect();
    for (i, pin) in all.iter().enumerate() {
        let comma = if i + 1 == all.len() { "" } else { "," };
        let _ = writeln!(t, "        .{}({}){comma}", pin.port, pin.port);
    }
    let _ = writeln!(t, "    );");

    // Opening the host's side.
    let mut open = String::new();
    let _ = writeln!(open, "\n    string path_file, page, port_file, description;\n    int port = 0;\n    initial begin");
    for (name, _) in resources.iter().filter(|(n, r)| r.kind == "button" && used(n)) {
        let _ = writeln!(open, "        {} = '0;", wrapper::find_pin(&pins, name, "in").port);
    }
    let _ = writeln!(open, "        if (!$value$plusargs(\"vboard_page=%s\", page)) page = \"vboard.html\";");
    let _ = writeln!(open, "        if (!$value$plusargs(\"vboard_port_file=%s\", port_file)) port_file = \"vboard.port\";");
    let _ = writeln!(open, "        if (!$value$plusargs(\"vboard_description=%s\", description)) description = \"panel.json\";");
    let _ = writeln!(open, "        void'($value$plusargs(\"vboard_port=%d\", port));");
    let _ = writeln!(
        open,
        "        if (vb_open(page, port_file, description, \"{}\", {mhz}, port, int'($test$plusargs(\"vboard_restart\"))) != 0) $fatal(1, \"no front panel\");",
        board.name()
    );
    for (name, r) in resources.iter().filter(|(n, _)| used(n)) {
        // The widget's bits: the LEDs' or buttons' count, a screen's colour.
        let width = pins.iter().find(|(n, _)| n == name).map_or(1, |(_, l)| l.iter().map(|p| p.width).max().unwrap_or(1));
        let style = if usage.bridged(name) { "bridge".to_owned() } else { r.style.clone().unwrap_or_default() };
        let doc = r.doc.clone().unwrap_or_default().replace('"', "'");
        let _ = writeln!(open, "        {name}_id = vb_declare(\"{}\", \"{name}\", {width}, \"{style}\", \"{doc}\");", r.kind);
    }
    if uart.is_some() {
        let _ = writeln!(open, "        if (!$value$plusargs(\"pty_path_file=%s\", path_file)) path_file = \"uart_pty.path\";");
        let _ = writeln!(open, "        if (uart_pty_open(path_file) != 0) $fatal(1, \"no pseudo-terminal\");");
    }
    let _ = writeln!(open, "    end");
    t.push_str(&open);

    for (name, r) in resources {
        let pin = |key: &str| wrapper::find_pin(&pins, name, key).port.clone();
        let _ = writeln!(t, "\n    // {name}: {}{}", r.kind, r.doc.as_ref().map(|d| format!(", {d}")).unwrap_or_default());
        if !used(name) {
            // Idle, as on a board with nothing attached: inputs at rest,
            // outputs unread, no widget on the panel.
            let _ = writeln!(t, "    // Unused by the design.");
            let rest = match r.kind.as_str() {
                "button" => format!("    assign {} = '0;", pin("in")),
                "uart" => format!("    assign {} = 1'b1;", pin("rx")),
                "spi" => format!("    assign {} = 1'b1;", pin("miso")),
                "i2c" => format!("    assign {} = !{};\n    assign {} = !{};", pin("scl_in"), pin("scl_oe"), pin("sda_in"), pin("sda_oe")),
                _ => String::new(),
            };
            if !rest.is_empty() {
                let _ = writeln!(t, "{rest}");
            }
            continue;
        }
        match r.kind.as_str() {
            "led" => {
                let out = pin("out");
                let w = wrapper::find_pin(&pins, name, "out").width;
                let _ = writeln!(
                    t,
                    "    logic [{}:0] {name}_last;\n    always @(posedge clk) if ({out} !== {name}_last) begin\n        vb_output({name}_id, int'({out}));\n        $display(\"{name} %b\", {out});\n        {name}_last <= {out};\n    end",
                    w - 1
                );
            }
            "button" => {
                let input = pin("in");
                let w = wrapper::find_pin(&pins, name, "in").width;
                let _ = writeln!(
                    t,
                    "    always @(posedge clk) if (tick == '0) begin\n        int v;\n        v = vb_input({name}_id);\n        {input} <= v[{}:0];\n    end",
                    w - 1
                );
            }
            "uart" if uart == Some(name.as_str()) => {
                let (rx, tx) = (pin("rx"), pin("tx"));
                let baud = u64::from(r.baud.unwrap_or(115_200));
                let clks = (mhz as u64 * 1_000_000 + baud / 2) / baud;
                let _ = writeln!(
                    t,
                    "    wire       {name}_from_valid;\n    wire [7:0] {name}_from;\n    logic      {name}_to_valid = 1'b0;\n    logic [7:0] {name}_to;\n    wire       {name}_to_idle;\n    logic [5:0] {name}_poll = '0;\n    uart_rx #(.CLKS_PER_BIT({clks})) {name}_host_rx\n    (\n        .clk(clk), .rst(rst), .rxd({tx}),\n        .valid_out({name}_from_valid), .data_out({name}_from), .framing_error_out()\n    );\n    uart_tx #(.CLKS_PER_BIT({clks})) {name}_host_tx\n    (\n        .clk(clk), .rst(rst), .valid_in({name}_to_valid), .data_in({name}_to),\n        .idle_out({name}_to_idle), .txd({rx})\n    );\n    // The pseudo-terminal first, then the console, every 64 cycles; the\n    // panel sees both ways.\n    always @(posedge clk) begin\n        if ({name}_from_valid) begin\n            uart_pty_putc({{24'd0, {name}_from}});\n            vb_uart_out({{24'd0, {name}_from}});\n        end\n        {name}_poll <= {name}_poll + 6'd1;\n        {name}_to_valid <= 1'b0;\n        if (!rst && {name}_to_idle && !{name}_to_valid && {name}_poll == '0) begin\n            int c;\n            c = uart_pty_getc();\n            if (c < 0) c = vb_uart_in();\n            if (c >= 0) begin\n                vb_uart_sent(c);\n                {name}_to <= c[7:0];\n                {name}_to_valid <= 1'b1;\n            end\n        end\n    end"
                );
            }
            "uart" => {
                // Only the first serial port the design uses reaches the
                // host; this one idles.
                let _ = writeln!(t, "    assign {} = 1'b1;", pin("rx"));
            }
            "spi" => {
                let (sclk, mosi, cs_n, miso) = (pin("sclk"), pin("mosi"), pin("cs_n"), pin("miso"));
                let _ = writeln!(
                    t,
                    "    spi_flash_model {name}_flash\n    (\n        .sclk({sclk}), .mosi({mosi}), .cs_n({cs_n}), .miso({miso}), .errors()\n    );\n    // Transactions, for the panel: each fall of the chip select.\n    logic {name}_cs_last = 1'b1;\n    int {name}_count = 0;\n    always @(posedge clk) begin\n        {name}_cs_last <= {cs_n};\n        if ({name}_cs_last && !{cs_n}) begin\n            {name}_count <= {name}_count + 1;\n            vb_output({name}_id, {name}_count + 1);\n        end\n    end"
                );
            }
            "i2c" => {
                let (scl_oe, scl_in, sda_oe, sda_in) = (pin("scl_oe"), pin("scl_in"), pin("sda_oe"), pin("sda_in"));
                let _ = writeln!(
                    t,
                    "    // Open drain with pull-ups: a line is low while anyone pulls it.\n    wire {name}_target_scl_oe, {name}_target_sda_oe;\n    assign {scl_in} = !({scl_oe} | {name}_target_scl_oe);\n    assign {sda_in} = !({sda_oe} | {name}_target_sda_oe);\n    i2c_target_model #(.ADDRESS(7'h48)) {name}_sensor\n    (\n        .clk(clk), .scl({scl_in}), .sda({sda_in}), .scl_oe({name}_target_scl_oe), .sda_oe({name}_target_sda_oe)\n    );\n    // Transactions, for the panel: each start, SDA falling while SCL is high.\n    logic {name}_sda_last = 1'b1;\n    int {name}_count = 0;\n    always @(posedge clk) begin\n        {name}_sda_last <= {sda_in};\n        if ({scl_in} && {name}_sda_last && !{sda_in}) begin\n            {name}_count <= {name}_count + 1;\n            vb_output({name}_id, {name}_count + 1);\n        end\n    end"
                );
            }
            "vga" => {
                let _ = writeln!(
                    t,
                    "    vga_capture #(\n        .H_VISIBLE({}), .H_SYNC({}), .H_BACK({}),\n        .V_VISIBLE({}), .V_SYNC({}), .V_BACK({})\n    ) {name}_capture\n    (\n        .clk(clk), .hsync({}), .vsync({}), .rgb({})\n    );",
                    VGA_H[0],
                    VGA_H[2],
                    VGA_H[3],
                    VGA_V[0],
                    VGA_V[2],
                    VGA_V[3],
                    pin("hsync"),
                    pin("vsync"),
                    pin("rgb")
                );
            }
            _ => {}
        }
    }
    let _ = writeln!(t, "endmodule\n\n`default_nettype wire");
    Ok(t)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn board(resources: &str) -> Board {
        let spec: kg_config::BoardSpec = toml::from_str(&format!(
            "[board]\nname = \"demo\"\ncore-module = \"demo_core\"\n[fpga]\ndevice = \"d\"\npart = \"p\"\nhal = \"ice40\"\ntool = \"virtual\"\n[clock]\nmhz = 25\n{resources}"
        ))
        .unwrap();
        Board { dir: "/b".into(), platform: "/p".into(), spec, project: Default::default() }
    }

    /// Every resource reaches the host, each by its own ports, with the
    /// screen read at the mode the core generates.
    #[test]
    fn each_resource_is_put_on_the_host() {
        let b = board(
            "[resources.led]\nkind = \"led\"\nwidth = 3\nstyle = \"rgb\"\npins = { out = \"rgb\" }\n\
             [resources.keys]\nkind = \"button\"\nwidth = 4\n\
             [resources.uart]\nkind = \"uart\"\nbaud = 115200\n\
             [resources.i2c]\nkind = \"i2c\"\n\
             [resources.vga]\nkind = \"vga\"\n",
        );
        let tb = generate(&b, &Usage::default()).unwrap();
        for line in [
            "demo_core dut",
            ".rgb(rgb),",
            "module tb (input logic clk);",
            "wire rst = por != 5'd16;",
            "led_id = vb_declare(\"led\", \"led\", 3, \"rgb\", \"\");",
            "keys_in <= v[3:0];",
            "uart_rx #(.CLKS_PER_BIT(217)) uart_host_rx",
            "assign i2c_scl_in = !(i2c_scl_oe | i2c_target_scl_oe);",
            ".H_VISIBLE(640), .H_SYNC(96), .H_BACK(48),",
            ".clk(clk), .hsync(vga_hsync), .vsync(vga_vsync), .rgb(vga_rgb)",
        ] {
            assert!(tb.contains(line), "missing `{line}` in\n{tb}");
        }
        let files: Vec<String> = support(&b, &Usage::default()).iter().map(|p| p.file_name().unwrap().to_owned()).collect();
        assert_eq!(files, ["main.cpp", "vboard.cpp", "uart_pty.cpp", "vga_capture.sv", "i2c_target_model.sv"]);
    }

    /// A generated core says what the design uses: only that reaches the
    /// panel, and the rest idle at rest, listed as unused.
    #[test]
    fn only_what_the_design_uses_is_on_the_panel() {
        let b = board(
            "[resources.led]\nkind = \"led\"\nwidth = 3\n\
             [resources.keys]\nkind = \"button\"\nwidth = 4\n\
             [resources.uart]\nkind = \"uart\"\n\
             [resources.spi]\nkind = \"spi\"\n\
             [resources.i2c]\nkind = \"i2c\"\n\
             [resources.vga]\nkind = \"vga\"\n",
        );
        let core = Core {
            module: "demo_core".to_owned(),
            text: String::new(),
            bindings: vec![("show".to_owned(), Target::Resource { resource: "led".to_owned(), role: String::new() })],
            bridge: None,
        };
        let usage = Usage { core: Some(&core), interface: None };
        let tb = generate(&b, &usage).unwrap();
        assert!(tb.contains("led_id = vb_declare(\"led\""), "{tb}");
        for name in ["keys", "uart", "spi", "i2c", "vga"] {
            assert!(!tb.contains(&format!("{name}_id = vb_declare")), "`{name}` declared in\n{tb}");
        }
        for line in ["assign keys_in = '0;", "assign uart_rx = 1'b1;", "assign spi_miso = 1'b1;", "assign i2c_scl_in = !i2c_scl_oe;"] {
            assert!(tb.contains(line), "missing `{line}` in\n{tb}");
        }
        assert!(!tb.contains("uart_pty_open(path_file)"), "{tb}");
        let files: Vec<String> = support(&b, &usage).iter().map(|p| p.file_name().unwrap().to_owned()).collect();
        assert_eq!(files, ["main.cpp", "vboard.cpp", "uart_pty.cpp"]);
        let description = describe(&b, &usage);
        let unused: Vec<&str> = description["unused"].as_array().unwrap().iter().map(|r| r["name"].as_str().unwrap()).collect();
        assert_eq!(unused, ["i2c", "keys", "spi", "uart", "vga"]);
        assert_eq!(description["resources"]["led"]["serves"][0]["function"], "show");
    }

    #[test]
    fn a_board_without_resources_names_its_own_bridge() {
        let err = generate(&board(""), &Usage::default()).unwrap_err();
        assert!(err.message().contains("names no bridge of its own"), "{}", err.message());
    }
}
