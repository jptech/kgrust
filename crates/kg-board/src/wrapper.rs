//! The generated core: a board's resources wired to a design's exported
//! functions, so a project needs no glue RTL (`docs/plans/board-tooling.md`
//! §6).
//!
//! A board declares its resources in `board.toml` (`[resources.<name>]`: a
//! kind, the core module's ports it is on, its parameters). Each kind has
//! roles, and each role a contract: the shapes of method or callback that
//! can serve it. A function binds to a role by the manifest's `[board]
//! connect` table or by its own name (`led`, `uart_rx`, `_uart_tx`; leading
//! underscores aside). Whatever binds to nothing is served by the host
//! bridge over the board's serial port, if the design leaves that free:
//! [`bridge`] has the protocol, `platform/python/kgbridge.py` the host side.
//!
//! A shape that fits no contract is refused with the contract spelled out,
//! never wired approximately. The generated module replaces a hand-written
//! core, and a project that names its own core module in `[board] rtl` gets
//! none.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use kg_config::{Board, board::Resource};

use crate::interface::{Function, Interface, Protocol, Value};
use crate::{Failure, Result};

/// The generated core, and what it bound.
#[derive(Debug, Clone)]
pub struct Core {
    pub module: String,
    pub text: String,
    /// Each function and what serves it, in the interface's order.
    pub bindings: Vec<(String, Target)>,
    pub bridge: Option<bridge::Plan>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    /// `resource.role`; the role is empty for a kind with one.
    Resource { resource: String, role: String },
    Bridge,
    /// `connect = "none"`: a method left uncalled.
    Unused,
}

impl std::fmt::Display for Target {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Target::Resource { resource, role } if role.is_empty() => write!(f, "{resource}"),
            Target::Resource { resource, role } => write!(f, "{resource}.{role}"),
            Target::Bridge => write!(f, "bridge"),
            Target::Unused => write!(f, "none"),
        }
    }
}

impl Core {
    /// For `kg board interface` and the build's log: a line a binding.
    pub fn summary(&self) -> String {
        let mut text = String::new();
        for (function, target) in &self.bindings {
            let _ = writeln!(text, "  {function:<20} -> {target}");
        }
        if let Some(plan) = &self.bridge {
            let _ = writeln!(text, "  bridge on {} at {} baud, interface hash {:08x}", plan.resource, plan.baud, plan.hash);
        }
        text
    }
}

/// What a resource's kind is: its roles, and its pins with their direction
/// and width (`0` for the resource's `width`).
struct Kind {
    roles: &'static [&'static str],
    pins: &'static [(&'static str, bool, u32)],
}

fn kind(name: &str) -> Option<Kind> {
    const IN: bool = true;
    const OUT: bool = false;
    Some(match name {
        "led" => Kind { roles: &[""], pins: &[("out", OUT, 0)] },
        "button" => Kind { roles: &[""], pins: &[("in", IN, 0)] },
        "uart" => Kind { roles: &["rx", "tx"], pins: &[("rx", IN, 1), ("tx", OUT, 1)] },
        "spi" => Kind { roles: &[""], pins: &[("sclk", OUT, 1), ("mosi", OUT, 1), ("miso", IN, 1), ("cs_n", OUT, 1)] },
        "i2c" => Kind { roles: &[""], pins: &[("scl_oe", OUT, 1), ("scl_in", IN, 1), ("sda_oe", OUT, 1), ("sda_in", IN, 1)] },
        "vga" => Kind { roles: &["pixel", "frame"], pins: &[("hsync", OUT, 1), ("vsync", OUT, 1), ("rgb", OUT, 12)] },
        _ => return None,
    })
}

/// The kinds, for an error that lists them.
const KINDS: &str = "led, button, uart, spi, i2c, vga";

/// One core port.
struct Pin {
    /// The kind's name for it: `rx`, `sclk`.
    key: &'static str,
    port: String,
    input: bool,
    width: u32,
}

/// Generates the core module for `board` around `interface`, binding by
/// `connect` (the manifest's) and by name.
pub fn generate(board: &Board, interface: &Interface) -> Result<Core> {
    let core = board
        .core_module()
        .ok_or_else(|| Failure::Usage(format!("the board {} names no core-module to generate", board.name())))?
        .to_owned();
    let resources = &board.spec.resources;
    if resources.is_empty() {
        return Err(Failure::Usage(format!(
            "the board {} declares no [resources.*] to generate a core from; name your core in [board] rtl",
            board.name()
        )));
    }
    let clock_hz = board.spec.clock.mhz.map(|m| m as u64 * 1_000_000);
    let mut pins: Vec<(String, Vec<Pin>)> = Vec::new();
    for (name, resource) in resources {
        let k = kind(&resource.kind).ok_or_else(|| {
            Failure::Usage(format!("[resources.{name}]: no kind `{}`; kinds are {KINDS}", resource.kind))
        })?;
        let mut list = Vec::new();
        for &(pin, input, width) in k.pins {
            let width = if width == 0 { resource.width.unwrap_or(1) } else { width };
            let port = resource.pins.get(pin).cloned().unwrap_or_else(|| format!("{name}_{pin}"));
            list.push(Pin { key: pin, port, input, width });
        }
        if let Some(stray) = resource.pins.keys().find(|p| !k.pins.iter().any(|(q, ..)| q == p)) {
            let known: Vec<&str> = k.pins.iter().map(|(p, ..)| *p).collect();
            return Err(Failure::Usage(format!(
                "[resources.{name}]: a {} has no pin `{stray}`; its pins are {}",
                resource.kind,
                known.join(", ")
            )));
        }
        pins.push((name.clone(), list));
    }

    let bindings = bind(board, interface)?;
    let mut g = Gen::new(&core, board.name(), &interface.module);
    g.ports(&pins);
    g.instance(interface);

    let mut bridged: Vec<&Function> = Vec::new();
    let mut used: BTreeMap<(String, String), &Function> = BTreeMap::new();
    for (function, target) in &bindings {
        let f = interface.function(function).expect("bound from the interface");
        match target {
            Target::Resource { resource, role } => {
                used.insert((resource.clone(), role.clone()), f);
            }
            Target::Bridge => bridged.push(f),
            Target::Unused => g.unused(f),
        }
    }

    let mut plan = None;
    if !bridged.is_empty() {
        let (uart, resource) = resources.iter().find(|(_, r)| r.kind == "uart").ok_or_else(|| {
            let names: Vec<&str> = bridged.iter().map(|f| f.name.as_str()).collect();
            Failure::Usage(format!(
                "{} bind to no resource of the board {}, and it has no serial port to bridge them over; bind them in [board] connect",
                names.join(", "),
                board.name()
            ))
        })?;
        if let Some(((_, role), f)) = used.iter().find(|((r, _), _)| r == uart) {
            let names: Vec<&str> = bridged.iter().map(|f| f.name.as_str()).collect();
            return Err(Failure::Usage(format!(
                "{} bind to no resource, and the bridge needs the serial port `{uart}`, which {} serves as {uart}.{role}; bind them in [board] connect, or leave the port to the bridge",
                names.join(", "),
                f.name
            )));
        }
        let hz = clock_hz.ok_or_else(|| no_clock(board, uart))?;
        let baud = resource.baud.unwrap_or(115_200);
        let p = bridge::plan(interface, &bridged, uart, baud)?;
        let pin_of = |pin: &str| find_pin(&pins, uart, pin).port.clone();
        g.bridge(&p, &bridged, divide(hz, u64::from(baud)), &pin_of("rx"), &pin_of("tx"));
        plan = Some(p);
    }

    for (name, resource) in resources {
        if plan.as_ref().is_some_and(|p| &p.resource == name) {
            continue;
        }
        let pin_of = |pin: &str| find_pin(&pins, name, pin).port.clone();
        let width_of = |pin: &str| find_pin(&pins, name, pin).width;
        let role = |role: &str| used.get(&(name.clone(), role.to_owned())).copied();
        let served: Vec<String> = used
            .iter()
            .filter(|((r, _), _)| r == name)
            .map(|((_, role), f)| if role.is_empty() { f.name.clone() } else { format!("{} as {role}", f.name) })
            .collect();
        let served = if served.is_empty() { "unused, idle".to_owned() } else { format!("served by {}", served.join(", ")) };
        g.section(&format!("{name}, a {}: {served}", resource.kind));
        let r = match resource.kind.as_str() {
            "led" => g.led(name, role(""), &pin_of("out"), width_of("out")),
            "button" => g.button(name, role(""), &pin_of("in"), width_of("in")),
            "uart" => {
                let hz = clock_hz.filter(|_| role("rx").is_some() || role("tx").is_some());
                let clks = hz.map(|hz| divide(hz, u64::from(resource.baud.unwrap_or(115_200))));
                if clks.is_none() && (role("rx").is_some() || role("tx").is_some()) {
                    return Err(no_clock(board, name));
                }
                g.uart(name, role("rx"), role("tx"), clks.unwrap_or(0), &pin_of("rx"), &pin_of("tx"))
            }
            "spi" => {
                let f = role("");
                let div = match f {
                    Some(_) => divide(clock_hz.ok_or_else(|| no_clock(board, name))?, 2 * u64::from(resource.hz.unwrap_or(1_000_000))),
                    None => 1,
                };
                g.spi(name, f, div.max(1), resource.mode.unwrap_or(0), [&pin_of("sclk"), &pin_of("mosi"), &pin_of("miso"), &pin_of("cs_n")])
            }
            "i2c" => {
                let f = role("");
                let div = match f {
                    Some(_) => divide(clock_hz.ok_or_else(|| no_clock(board, name))?, 4 * u64::from(resource.hz.unwrap_or(100_000))),
                    None => 1,
                };
                g.i2c(name, f, div.max(1), [&pin_of("scl_oe"), &pin_of("scl_in"), &pin_of("sda_oe"), &pin_of("sda_in")])
            }
            "vga" => g.vga(name, role("pixel"), role("frame"), [&pin_of("hsync"), &pin_of("vsync"), &pin_of("rgb")]),
            _ => unreachable!("kinds are checked above"),
        };
        r.map_err(Failure::Usage)?;
    }
    Ok(Core { module: core, text: g.finish(), bindings, bridge: plan })
}

fn no_clock(board: &Board, resource: &str) -> Failure {
    Failure::Usage(format!("the board {} gives no [clock] mhz, which `{resource}` needs for its timing", board.name()))
}

fn find_pin<'a>(pins: &'a [(String, Vec<Pin>)], resource: &str, key: &str) -> &'a Pin {
    pins.iter()
        .find(|(r, _)| r == resource)
        .and_then(|(_, list)| list.iter().find(|p| p.key == key))
        .expect("every kind's pins are listed")
}

/// `a / b`, rounded to the nearest.
fn divide(a: u64, b: u64) -> u64 {
    (a + b / 2) / b.max(1)
}

/// Every function's target: the manifest's `connect`, else its name, else
/// the bridge.
pub fn bind(board: &Board, interface: &Interface) -> Result<Vec<(String, Target)>> {
    let resources = &board.spec.resources;
    let connect = &board.project.connect;
    if let Some(stray) = connect.keys().find(|k| interface.function(k).is_none()) {
        let names: Vec<&str> = interface.methods.iter().chain(&interface.callbacks).map(|f| f.name.as_str()).collect();
        return Err(Failure::Usage(format!(
            "[board] connect names `{stray}`, which {} does not export; it has {}",
            interface.module,
            names.join(", ")
        )));
    }
    let mut out: Vec<(String, Target)> = Vec::new();
    for f in interface.methods.iter().chain(&interface.callbacks) {
        let target = match connect.get(&f.name) {
            Some(text) => parse_target(text, resources).map_err(|e| Failure::Usage(format!("[board] connect {} = \"{text}\": {e}", f.name)))?,
            None => by_name(&f.name, resources).unwrap_or(Target::Bridge),
        };
        if let Target::Resource { resource, role } = &target
            && let Some((other, _)) = out.iter().find(|(_, t)| t == &target)
        {
            let at = if role.is_empty() { resource.clone() } else { format!("{resource}.{role}") };
            return Err(Failure::Usage(format!("{other} and {} both bind to {at}; one function serves a role", f.name)));
        }
        out.push((f.name.clone(), target));
    }
    Ok(out)
}

fn parse_target(text: &str, resources: &BTreeMap<String, Resource>) -> std::result::Result<Target, String> {
    match text {
        "bridge" => return Ok(Target::Bridge),
        "none" => return Ok(Target::Unused),
        _ => {}
    }
    let (resource, role) = text.split_once('.').unwrap_or((text, ""));
    let r = resources.get(resource).ok_or_else(|| {
        let names: Vec<&str> = resources.keys().map(String::as_str).collect();
        format!("no resource `{resource}`; the board has {}, or \"bridge\", or \"none\"", names.join(", "))
    })?;
    let k = kind(&r.kind).ok_or_else(|| format!("no kind `{}`", r.kind))?;
    let role = match (role, k.roles) {
        ("", [only]) => (*only).to_owned(),
        ("", roles) if r.kind == "vga" => roles[0].to_owned(),
        ("", roles) => return Err(format!("a {} has roles {}; name one, `{resource}.{}`", r.kind, roles.join(", "), roles[0])),
        (role, roles) if roles.contains(&role) => role.to_owned(),
        (role, roles) => return Err(format!("a {} has no role `{role}`; it has {}", r.kind, roles.join(", "))),
    };
    Ok(Target::Resource { resource: resource.to_owned(), role })
}

/// `led`, `uart_rx`, `_uart_tx`, `vga` (its first role), `vga_frame`.
fn by_name(name: &str, resources: &BTreeMap<String, Resource>) -> Option<Target> {
    let bare = name.trim_start_matches('_');
    for (resource, r) in resources {
        let k = kind(&r.kind)?;
        for (i, role) in k.roles.iter().enumerate() {
            let full = if role.is_empty() { resource.clone() } else { format!("{resource}_{role}") };
            if bare == full || (i == 0 && k.roles.len() > 1 && r.kind == "vga" && bare == resource) {
                return Some(Target::Resource { resource: resource.clone(), role: (*role).to_owned() });
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------
// The text.

struct Gen {
    text: String,
    body: String,
    module: String,
    /// The bridge's calls out to the host from FIFO callbacks: when, the
    /// frame's length and bytes, and the FIFO's read enable.
    pending_calls: Vec<(String, String, String, Option<String>)>,
}

impl Gen {
    fn new(core: &str, board: &str, module: &str) -> Gen {
        let mut text = String::new();
        let _ = writeln!(text, "// Generated by `kg board` for the {board} board from {module}'s interface");
        let _ = writeln!(text, "// (target/board/interface.json) and the board's [resources]. Do not edit: a");
        let _ = writeln!(text, "// project replaces it by naming its own `{core}` in [board] rtl.");
        let _ = writeln!(text, "\n`default_nettype none\n");
        Gen { text, body: String::new(), module: core.to_owned(), pending_calls: Vec::new() }
    }

    fn ports(&mut self, pins: &[(String, Vec<Pin>)]) {
        let _ = writeln!(self.text, "module {}\n(\n    input  wire clk,\n    input  wire rst,", self.module);
        let all: Vec<&Pin> = pins.iter().flat_map(|(_, l)| l).collect();
        for (i, pin) in all.iter().enumerate() {
            let dir = if pin.input { "input  wire" } else { "output wire" };
            let range = if pin.width > 1 { format!("[{}:0] ", pin.width - 1) } else { String::new() };
            let comma = if i + 1 == all.len() { "" } else { "," };
            let _ = writeln!(self.text, "    {dir} {range}{}{comma}", pin.port);
        }
        let _ = writeln!(self.text, ");");
    }

    fn instance(&mut self, interface: &Interface) {
        let mut wires = String::new();
        let mut conns = vec![
            ".clk                        (clk)".to_owned(),
            ".rst                        (rst)".to_owned(),
            ".rst_and_startup_done_out   (started)".to_owned(),
        ];
        for f in interface.methods.iter().chain(&interface.callbacks) {
            for (port, width) in ports_of(f) {
                let range = if width > 1 { format!("[{}:0] ", width - 1) } else { String::new() };
                let _ = writeln!(wires, "    logic {range}{port};");
                conns.push(format!(".{port:<26} ({port})"));
            }
        }
        conns.push(".stall_rate_supported_out   ()".to_owned());
        conns.push(".stall_rate_valid_in        (1'b0)".to_owned());
        conns.push(".stall_rate_in              (3'd0)".to_owned());
        let _ = writeln!(self.text, "    // Out of reset and started: nothing is called before.\n    wire started;\n");
        self.text.push_str(&wires);
        let _ = writeln!(self.text, "\n    {} exported\n    (", interface.module);
        let _ = writeln!(self.text, "        {}", conns.join(",\n        "));
        let _ = writeln!(self.text, "    );");
    }

    fn section(&mut self, title: &str) {
        let _ = writeln!(self.body, "\n    // {title}");
    }

    fn line(&mut self, text: &str) {
        let _ = writeln!(self.body, "    {text}");
    }

    fn finish(mut self) -> String {
        self.text.push_str(&self.body);
        let _ = writeln!(self.text, "endmodule\n\n`default_nettype wire");
        self.text
    }

    /// `connect = "none"`: never called; a callback is never answered.
    fn unused(&mut self, f: &Function) {
        self.section(&format!("{}: unused (connect = \"none\")", f.name));
        for (port, _) in inputs_of(f) {
            self.line(&format!("assign {port} = '0;"));
        }
    }

    // --- led -----------------------------------------------------------

    fn led(&mut self, name: &str, f: Option<&Function>, out: &str, width: u32) -> std::result::Result<(), String> {
        let Some(f) = f else {
            self.line(&format!("assign {out} = '0;"));
            return Ok(());
        };
        let contract = format!(
            "`{name}` ({width} LED{}) takes a [[latency(0)]] method with no arguments returning up to uint{width}, called every cycle, or a callback taking one value of up to {width} bits",
            if width == 1 { "" } else { "s" }
        );
        let is_method = !f.callback;
        match (is_method, f.protocol, f.args.len(), &f.result) {
            (true, Protocol::Fixed, 0, Some(r)) if r.width <= width => {
                self.call_every_cycle(f);
                self.line(&format!("assign {out} = {};", widen(port(&f.ports.result), r.width, width)));
                Ok(())
            }
            (false, _, 1, None) if f.args[0].width <= width => {
                let reg = format!("{name}_value");
                self.line(&format!("logic [{}:0] {reg};", width - 1));
                let take = self.take_callback(f);
                self.line(&format!(
                    "always_ff @(posedge clk) if (rst) {reg} <= '0; else if ({take}) {reg} <= {};",
                    widen(&f.ports.args[0], f.args[0].width, width)
                ));
                self.line(&format!("assign {out} = {reg};"));
                Ok(())
            }
            _ => Err(mismatch(f, &contract)),
        }
    }

    // --- button --------------------------------------------------------

    fn button(&mut self, name: &str, f: Option<&Function>, pin: &str, width: u32) -> std::result::Result<(), String> {
        let Some(f) = f else { return Ok(()) };
        let contract = format!(
            "`{name}` ({width} button{}) takes a callback with no arguments returning uint{width} or wider, read when called, or a method taking one uint{width} or wider, called once started and on every change",
            if width == 1 { "" } else { "s" }
        );
        // Two flip-flops: the pins are asynchronous to the clock.
        let (meta, now) = (format!("{name}_meta"), format!("{name}_now"));
        let range = format!("[{}:0]", width - 1);
        let sync = |g: &mut Gen| {
            g.line(&format!("logic {range} {meta}, {now};"));
            g.line(&format!("always_ff @(posedge clk) begin {meta} <= {pin}; {now} <= {meta}; end"));
        };
        let is_method = !f.callback;
        match (is_method, f.protocol, f.args.len(), &f.result) {
            (false, Protocol::Fixed | Protocol::Handshake, 0, Some(r)) if r.width >= width => {
                sync(self);
                if f.protocol == Protocol::Handshake {
                    self.answer_at_once(f);
                }
                self.line(&format!("assign {} = {};", port(&f.ports.result), widen(&now, width, r.width)));
                Ok(())
            }
            (true, Protocol::Fixed | Protocol::NoBackpressure | Protocol::Handshake, 1, None) if f.args[0].width >= width => {
                sync(self);
                let (last, primed, fire) = (format!("{name}_last"), format!("{name}_primed"), format!("{name}_call"));
                self.line(&format!("logic {range} {last};\n    logic {primed};"));
                let ready = if f.protocol == Protocol::Handshake { format!(" && {}", port(&f.ports.rdy_out)) } else { String::new() };
                self.line(&format!("wire {fire} = started && (!{primed} || {now} != {last}){ready};"));
                self.line(&format!(
                    "always_ff @(posedge clk) if (rst) {primed} <= 1'b0; else if ({fire}) begin {primed} <= 1'b1; {last} <= {now}; end"
                ));
                self.line(&format!("assign {} = {fire};", port(&f.ports.valid_in)));
                self.line(&format!("assign {} = {};", f.ports.args[0], widen(&now, width, f.args[0].width)));
                self.drain(f);
                Ok(())
            }
            _ => Err(mismatch(f, &contract)),
        }
    }

    // --- uart ----------------------------------------------------------

    fn uart(&mut self, name: &str, rx: Option<&Function>, tx: Option<&Function>, clks: u64, rxd: &str, txd: &str) -> std::result::Result<(), String> {
        if let Some(f) = rx {
            let contract = format!(
                "`{name}.rx` takes a method taking one uint8, called for each byte received ([[no_backpressure]] or [[latency(N)]]; with a handshake, a byte arriving while it is not ready is dropped), or a callback with no arguments returning uint8, answered with the next byte"
            );
            let (valid, data) = (format!("{name}_rx_valid"), format!("{name}_rx_byte"));
            self.line(&format!("wire {valid};\n    wire [7:0] {data};"));
            self.line(&format!(
                "uart_rx #(.CLKS_PER_BIT({clks})) {name}_receiver\n    (\n        .clk(clk), .rst(rst), .rxd({rxd}),\n        .valid_out({valid}), .data_out({data}), .framing_error_out()\n    );"
            ));
            match (!f.callback, f.protocol, f.args.len(), &f.result) {
                (true, Protocol::NoBackpressure | Protocol::Fixed, 1, None) if f.args[0].width == 8 => {
                    self.line(&format!("assign {} = {valid} & started;", port(&f.ports.valid_in)));
                    self.line(&format!("assign {} = {data};", f.ports.args[0]));
                }
                (true, Protocol::Handshake, 1, None) if f.args[0].width == 8 => {
                    self.line(&format!("assign {} = {valid} & started & {};", port(&f.ports.valid_in), port(&f.ports.rdy_out)));
                    self.line(&format!("assign {} = {data};", f.ports.args[0]));
                    self.drain(f);
                }
                (false, Protocol::Handshake, 0, Some(r)) if r.width == 8 => {
                    // One byte held for a caller not yet waiting; a second
                    // arriving first replaces it, as a UART overruns.
                    let (have, held) = (format!("{name}_rx_have"), format!("{name}_rx_held"));
                    let take = format!("{name}_rx_take");
                    self.line(&format!("logic {have};\n    logic [7:0] {held};"));
                    self.line(&format!(
                        "wire {take} = !{} && {have} && {};",
                        port(&f.ports.empty_out),
                        port(&f.ports.rdy_out)
                    ));
                    self.line(&format!(
                        "always_ff @(posedge clk) begin\n        if (rst) {have} <= 1'b0;\n        else if ({valid}) begin {have} <= 1'b1; {held} <= {data}; end\n        else if ({take}) {have} <= 1'b0;\n    end"
                    ));
                    self.line(&format!("assign {} = {take};", port(&f.ports.rden_in)));
                    self.line(&format!("assign {} = {take};", port(&f.ports.valid_in)));
                    self.line(&format!("assign {} = {held};", port(&f.ports.result)));
                }
                _ => return Err(mismatch(f, &contract)),
            }
        }
        let Some(f) = tx else {
            self.line(&format!("assign {txd} = 1'b1;"));
            return Ok(());
        };
        let contract = format!(
            "`{name}.tx` takes a [[latency(0)]] method with no arguments returning uint9, asked whenever the transmitter is idle (bit 8: a byte to send), or a callback taking one uint8 (with a handshake it waits for the transmitter; without, a byte sent while busy is dropped)"
        );
        let (idle, start, data) = (format!("{name}_tx_idle"), format!("{name}_tx_start"), format!("{name}_tx_data"));
        self.line(&format!("wire {idle};\n    wire {start};\n    wire [7:0] {data};"));
        match (!f.callback, f.protocol, f.args.len(), &f.result) {
            (true, Protocol::Fixed, 0, Some(r)) if f.latency == Some(0) && r.width == 9 => {
                let result = port(&f.ports.result);
                self.line(&format!("assign {} = {idle} & started;", port(&f.ports.valid_in)));
                self.line(&format!("assign {start} = {idle} & started & {result}[8];"));
                self.line(&format!("assign {data} = {result}[7:0];"));
            }
            (false, Protocol::Handshake | Protocol::Async, 1, None) if f.args[0].width == 8 => {
                let ready = if f.protocol == Protocol::Handshake { format!(" && {}", port(&f.ports.rdy_out)) } else { String::new() };
                self.line(&format!("assign {start} = !{} && {idle}{ready};", port(&f.ports.empty_out)));
                self.line(&format!("assign {} = {start};", port(&f.ports.rden_in)));
                if f.protocol == Protocol::Handshake {
                    self.line(&format!("assign {} = {start};", port(&f.ports.valid_in)));
                }
                self.line(&format!("assign {data} = {};", f.ports.args[0]));
            }
            (false, Protocol::NoBackpressure | Protocol::Fixed, 1, None) if f.args[0].width == 8 => {
                self.line(&format!("assign {start} = {} && {idle};", port(&f.ports.valid_out)));
                self.line(&format!("assign {data} = {};", f.ports.args[0]));
            }
            _ => return Err(mismatch(f, &contract)),
        }
        self.line(&format!(
            "uart_tx #(.CLKS_PER_BIT({clks})) {name}_transmitter\n    (\n        .clk(clk), .rst(rst), .valid_in({start}), .data_in({data}),\n        .idle_out({idle}), .txd({txd})\n    );"
        ));
        Ok(())
    }

    // --- spi, i2c ------------------------------------------------------

    fn spi(&mut self, name: &str, f: Option<&Function>, div: u64, mode: u8, [sclk, mosi, miso, cs_n]: [&str; 4]) -> std::result::Result<(), String> {
        let (cpol, cpha) = (mode >> 1 & 1, mode & 1);
        let Some(f) = f else {
            let _ = miso;
            self.line(&format!("assign {sclk} = 1'b{cpol};\n    assign {mosi} = 1'b0;\n    assign {cs_n} = 1'b1;"));
            return Ok(());
        };
        let contract = format!(
            "`{name}` takes a callback `(uint8 mosi, bool hold) -> uint8`: one byte out, the byte read back; chip select stays low while `hold`"
        );
        let fits = f.callback
            && f.protocol == Protocol::Handshake
            && f.args.iter().map(|a| a.width).collect::<Vec<_>>() == [8, 1]
            && f.result.as_ref().is_some_and(|r| r.width == 8);
        if !fits {
            return Err(mismatch(f, &contract));
        }
        self.line(&format!(
            "spi_master #(.CLK_DIV({div}), .CPOL(1'b{cpol}), .CPHA(1'b{cpha})) {name}_engine\n    (\n        .clk(clk), .rst(rst),\n        .call_empty({}), .call_rden({}), .call_mosi({}), .call_hold({}),\n        .result_valid({}), .result({}), .result_rdy({}),\n        .sclk({sclk}), .mosi({mosi}), .miso({miso}), .cs_n({cs_n})\n    );",
            port(&f.ports.empty_out),
            port(&f.ports.rden_in),
            f.ports.args[0],
            f.ports.args[1],
            port(&f.ports.valid_in),
            port(&f.ports.result),
            port(&f.ports.rdy_out)
        ));
        Ok(())
    }

    fn i2c(&mut self, name: &str, f: Option<&Function>, div: u64, [scl_oe, scl_in, sda_oe, sda_in]: [&str; 4]) -> std::result::Result<(), String> {
        let Some(f) = f else {
            let _ = (scl_in, sda_in);
            self.line(&format!("assign {scl_oe} = 1'b0;\n    assign {sda_oe} = 1'b0;"));
            return Ok(());
        };
        let contract = format!(
            "`{name}` takes a callback `(uint2 op, uint8 data) -> uint9`: op 0 start, 1 stop, 2 write (bit 8 of the result: not acknowledged), 3 read (data bit 0: acknowledge it)"
        );
        let fits = f.callback
            && f.protocol == Protocol::Handshake
            && f.args.iter().map(|a| a.width).collect::<Vec<_>>() == [2, 8]
            && f.result.as_ref().is_some_and(|r| r.width == 9);
        if !fits {
            return Err(mismatch(f, &contract));
        }
        self.line(&format!(
            "i2c_master #(.CLK_DIV({div})) {name}_engine\n    (\n        .clk(clk), .rst(rst),\n        .call_empty({}), .call_rden({}), .call_op({}), .call_data({}),\n        .result_valid({}), .result({}), .result_rdy({}),\n        .scl_oe({scl_oe}), .scl_in({scl_in}), .sda_oe({sda_oe}), .sda_in({sda_in})\n    );",
            port(&f.ports.empty_out),
            port(&f.ports.rden_in),
            f.ports.args[0],
            f.ports.args[1],
            port(&f.ports.valid_in),
            port(&f.ports.result),
            port(&f.ports.rdy_out)
        ));
        Ok(())
    }

    // --- vga -----------------------------------------------------------

    fn vga(&mut self, name: &str, pixel: Option<&Function>, frame: Option<&Function>, [hsync, vsync, rgb]: [&str; 3]) -> std::result::Result<(), String> {
        let Some(p) = pixel else {
            if let Some(f) = frame {
                return Err(format!("{} serves `{name}.frame`, but nothing serves `{name}.pixel`", f.name));
            }
            self.line(&format!("assign {hsync} = 1'b1;\n    assign {vsync} = 1'b1;\n    assign {rgb} = '0;"));
            return Ok(());
        };
        let contract = format!(
            "`{name}.pixel` takes a [[latency(N)]] method `(uint10 x, uint10 y)` returning uint12 ({{r, g, b}}, 4 bits each) or uint3 (1 bit each), called for every visible pixel"
        );
        let fits = !p.callback
            && p.protocol == Protocol::Fixed
            && p.args.len() == 2
            && p.args.iter().all(|a| a.width >= 10)
            && p.result.as_ref().is_some_and(|r| r.width == 12 || r.width == 3);
        if !fits {
            return Err(mismatch(p, &contract));
        }
        let latency = p.latency.unwrap_or(0);
        let (x, y, active, hs, vs, first) =
            (format!("{name}_x"), format!("{name}_y"), format!("{name}_active"), format!("{name}_hs"), format!("{name}_vs"), format!("{name}_frame"));
        self.line(&format!("wire [9:0] {x}, {y};\n    wire {active}, {hs}, {vs}, {first};"));
        self.line(&format!(
            "vga_timing {name}_timing\n    (\n        .clk(clk), .rst(rst | !started),\n        .x_out({x}), .y_out({y}), .active_out({active}),\n        .hsync_out({hs}), .vsync_out({vs}), .frame_out({first})\n    );"
        ));
        self.line(&format!("assign {} = {active};", port(&p.ports.valid_in)));
        self.line(&format!("assign {} = {};", p.ports.args[0], widen(&x, 10, p.args[0].width)));
        self.line(&format!("assign {} = {};", p.ports.args[1], widen(&y, 10, p.args[1].width)));
        // The colour comes `latency` cycles after its pixel's call: the sync
        // and blanking wait as long.
        let result = port(&p.ports.result);
        let colour = if p.result.as_ref().is_some_and(|r| r.width == 3) {
            format!("{{{{4{{{result}[2]}}}}, {{4{{{result}[1]}}}}, {{4{{{result}[0]}}}}}}")
        } else {
            result.to_owned()
        };
        if latency == 0 {
            self.line(&format!("assign {hsync} = {hs};\n    assign {vsync} = {vs};\n    assign {rgb} = {active} ? {colour} : 12'h000;"));
        } else {
            let delay = format!("{name}_delay");
            self.line(&format!("logic [2:0] {delay} [{latency}];"));
            self.line(&format!(
                "always_ff @(posedge clk) begin\n        {delay}[0] <= {{{active}, {hs}, {vs}}};\n        for (int i = 1; i < {latency}; i++) {delay}[i] <= {delay}[i - 1];\n    end"
            ));
            let last = format!("{delay}[{}]", latency - 1);
            self.line(&format!("assign {hsync} = {last}[1];\n    assign {vsync} = {last}[0];\n    assign {rgb} = {last}[2] ? {colour} : 12'h000;"));
        }
        if let Some(f) = frame {
            let contract = format!("`{name}.frame` takes a method with no arguments, called once a frame as the visible area ends");
            match (!f.callback, f.protocol, f.args.len()) {
                (true, Protocol::NoBackpressure | Protocol::Fixed, 0) => {
                    self.line(&format!("assign {} = {first};", port(&f.ports.valid_in)));
                }
                (true, Protocol::Handshake, 0) => {
                    // A frame whose call cannot start is skipped.
                    self.line(&format!("assign {} = {first} & {};", port(&f.ports.valid_in), port(&f.ports.rdy_out)));
                    self.drain(f);
                }
                _ => return Err(mismatch(f, &contract)),
            }
        }
        Ok(())
    }

    // --- shared --------------------------------------------------------

    fn call_every_cycle(&mut self, f: &Function) {
        self.line(&format!("assign {} = started;", port(&f.ports.valid_in)));
    }

    /// The cycle a callback's call is taken: its pulse, or its FIFO popped
    /// (and, with a handshake, its completion pushed in the same cycle).
    fn take_callback(&mut self, f: &Function) -> String {
        match f.protocol {
            Protocol::NoBackpressure | Protocol::Fixed => port(&f.ports.valid_out).to_owned(),
            Protocol::Async => {
                self.line(&format!("assign {} = !{};", port(&f.ports.rden_in), port(&f.ports.empty_out)));
                format!("!{}", port(&f.ports.empty_out))
            }
            Protocol::Handshake => {
                self.answer_at_once(f);
                port(&f.ports.rden_in).to_owned()
            }
        }
    }

    /// A handshake callback answered in the cycle it is popped.
    fn answer_at_once(&mut self, f: &Function) {
        let rden = port(&f.ports.rden_in);
        self.line(&format!("assign {rden} = !{} && {};", port(&f.ports.empty_out), port(&f.ports.rdy_out)));
        self.line(&format!("assign {} = {rden};", port(&f.ports.valid_in)));
    }

    /// A handshake method's completions, read and dropped.
    fn drain(&mut self, f: &Function) {
        if let (Some(rden), Some(empty)) = (&f.ports.rden_in, &f.ports.empty_out) {
            self.line(&format!("assign {rden} = !{empty};"));
        }
    }

    // --- the host bridge ----------------------------------------------

    fn bridge(&mut self, plan: &bridge::Plan, bridged: &[&Function], clks: u64, rxd: &str, txd: &str) {
        self.section(&format!(
            "{}: the host bridge ({} baud), serving {}",
            plan.resource,
            plan.baud,
            bridged.iter().map(|f| f.name.as_str()).collect::<Vec<_>>().join(", ")
        ));
        let rxb = plan.rx_bytes;
        let txb = plan.tx_bytes;
        let txw = 8 * txb;
        for line in [
            "wire link_rx_valid;".to_owned(),
            "wire [7:0] link_rx_len;".to_owned(),
            format!("wire [{}:0] link_rx_data;", 8 * rxb - 1),
            "wire link_rx_lost;".to_owned(),
            "wire link_tx_busy;".to_owned(),
            "wire [7:0] rx_op = link_rx_data[7:0];".to_owned(),
            format!(
                "kg_bridge_link #(.CLKS_PER_BIT({clks}), .RX_BYTES({rxb}), .TX_BYTES({txb})) link\n    (\n        .clk(clk), .rst(rst), .rxd({rxd}), .txd({txd}),\n        .rx_valid_out(link_rx_valid), .rx_len_out(link_rx_len), .rx_data_out(link_rx_data),\n        .rx_take_in(rx_take), .rx_lost_out(link_rx_lost),\n        .tx_start_in(tx_start), .tx_len_in(tx_len), .tx_data_in(tx_data), .tx_busy_out(link_tx_busy)\n    );"
            ),
            // One reply waits at a time, and one call out to the host.
            "logic reply_full, call_full;".to_owned(),
            "logic [7:0] reply_len, call_len;".to_owned(),
            format!("logic [{}:0] reply_data, call_data;", txw - 1),
            "logic lost;".to_owned(),
        ] {
            self.line(&line);
        }

        // Requests: a method call, a callback's answer, a hello.
        let mut takes: Vec<String> = Vec::new();
        let mut known: Vec<String> = Vec::new();
        let mut replies: Vec<(String, String, String)> = Vec::new(); // (when, len, data)
        let mut pulses: Vec<(String, String, String)> = Vec::new();
        for f in bridged.iter().filter(|f| !f.callback) {
            let op = f.index;
            let request = 1 + f.args.iter().map(Value::bytes).sum::<u32>();
            let call = format!("call_{}", f.name);
            known.push(format!("(rx_op == 8'd{op} && link_rx_len == 8'd{request})"));
            let ready = if f.protocol == Protocol::Handshake { format!(" && {}", port(&f.ports.rdy_out)) } else { String::new() };
            self.line(&format!("// {}: method {op}, {} request bytes", f.name, request));
            self.line(&format!("wire {call} = started && link_rx_valid && rx_op == 8'd{op} && link_rx_len == 8'd{request}{ready};"));
            self.line(&format!("assign {} = {call};", port(&f.ports.valid_in)));
            takes.push(call.clone());
            let mut offset = 1;
            for (arg, port_name) in f.args.iter().zip(&f.ports.args) {
                self.line(&format!("assign {port_name} = link_rx_data[{} +: {}];", 8 * offset, arg.width));
                offset += arg.bytes();
            }
            let rbytes = f.result.as_ref().map_or(0, Value::bytes);
            let data = frame_data(op as u32, f.result.as_ref().map(|r| (port(&f.ports.result).to_owned(), r.width, r.bytes())).into_iter().collect(), txw);
            let len = format!("8'd{}", 1 + rbytes);
            match f.protocol {
                Protocol::Handshake => {
                    replies.push((format!("!{}", port(&f.ports.empty_out)), len, data));
                    let _ = f.ports.rden_in.as_ref().expect("a handshake method returns through a FIFO");
                }
                Protocol::NoBackpressure => {
                    if let Some(v) = &f.ports.valid_out {
                        pulses.push((v.clone(), len, data));
                    } else {
                        // Nothing comes back: answered once called.
                        pulses.push((call.clone(), "8'd1".to_owned(), frame_data(op as u32, Vec::new(), txw)));
                    }
                }
                Protocol::Fixed | Protocol::Async => {
                    let n = f.latency.unwrap_or(0);
                    let due = if n == 0 {
                        call.clone()
                    } else {
                        let pipe = format!("due_{}", f.name);
                        self.line(&format!("logic [{}:0] {pipe};", n - 1));
                        let shift = if n == 1 { call.clone() } else { format!("{{{pipe}[{}:0], {call}}}", n - 2) };
                        self.line(&format!("always_ff @(posedge clk) if (rst) {pipe} <= '0; else {pipe} <= {shift};"));
                        format!("{pipe}[{}]", n - 1)
                    };
                    pulses.push((due, len, data));
                }
            }
        }
        for f in bridged.iter().filter(|f| f.callback) {
            let op = 0x80 | f.index as u32;
            self.line(&format!("// {}: callback {}, sent to the host as 0x{op:02x}", f.name, f.index));
            let args: Vec<(String, u32, u32)> =
                f.args.iter().zip(&f.ports.args).map(|(a, p)| (p.clone(), a.width, a.bytes())).collect();
            let len = format!("8'd{}", 1 + args.iter().map(|a| a.2).sum::<u32>());
            let data = frame_data(op, args, txw);
            match f.protocol {
                Protocol::Handshake | Protocol::Async => {
                    let when = format!("!{}", port(&f.ports.empty_out));
                    self.call_out(f, &when, &len, &data);
                }
                Protocol::NoBackpressure | Protocol::Fixed => {
                    pulses.push((format!("{} /* -> call */", port(&f.ports.valid_out)), len, data));
                    if let Some(result) = &f.ports.result {
                        // The host cannot answer within a fixed latency.
                        self.line(&format!("assign {result} = '0;"));
                    }
                }
            }
            if f.protocol == Protocol::Handshake {
                let answer = 1 + f.result.as_ref().map_or(0, Value::bytes);
                let answered = format!("answer_{}", f.name.trim_start_matches('_'));
                known.push(format!("(rx_op == 8'h{op:02x} && link_rx_len == 8'd{answer})"));
                self.line(&format!(
                    "wire {answered} = link_rx_valid && rx_op == 8'h{op:02x} && link_rx_len == 8'd{answer} && {};",
                    port(&f.ports.rdy_out)
                ));
                self.line(&format!("assign {} = {answered};", port(&f.ports.valid_in)));
                if let (Some(result), Some(r)) = (&f.ports.result, &f.result) {
                    self.line(&format!("assign {result} = link_rx_data[8 +: {}];", r.width));
                }
                takes.push(answered);
            }
        }

        // Hello: the protocol's version and the interface's hash, so the
        // host knows it speaks to this build.
        known.push("(rx_op == 8'hff && link_rx_len == 8'd1)".to_owned());
        self.line("wire hello = link_rx_valid && rx_op == 8'hff && link_rx_len == 8'd1;");
        let known_expr = if known.is_empty() { "1'b0".to_owned() } else { known.join("\n        || ") };
        self.line(&format!("wire known =\n        {known_expr};"));
        self.line("wire refused = link_rx_valid && !known;");
        // An unknown op, or a known one with the wrong length.
        let ops: Vec<String> = bridged
            .iter()
            .map(|f| if !f.callback { format!("8'd{}", f.index) } else { format!("8'h{:02x}", 0x80 | f.index) })
            .chain(["8'hff".to_owned()])
            .collect();
        let op_known = ops.iter().map(|o| format!("rx_op == {o}")).collect::<Vec<_>>().join(" || ");
        self.line(&format!("wire [7:0] refusal = ({op_known}) ? 8'd2 : 8'd1;"));

        // Replies, one buffer: pulses first (they cannot wait), then the
        // hello, a refusal and a lost frame, then FIFO results.
        let mut chain: Vec<(String, String, String, Option<String>)> = Vec::new(); // when, len, data, rden to assert
        for (when, len, data) in &pulses {
            if when.ends_with("/* -> call */") {
                continue;
            }
            chain.push((when.clone(), len.clone(), data.clone(), None));
        }
        chain.push(("hello".to_owned(), "8'd6".to_owned(), format!("{txw}'({{32'h{:08x}, 8'd{}, 8'hff}})", plan.hash, bridge::VERSION), None));
        chain.push(("refused".to_owned(), "8'd3".to_owned(), format!("{txw}'({{rx_op, refusal, 8'hfe}})"), None));
        chain.push(("lost".to_owned(), "8'd3".to_owned(), format!("{txw}'({{8'h00, 8'd3, 8'hfe}})"), None));
        for (f, (when, len, data)) in bridged
            .iter()
            .filter(|f| !f.callback && f.protocol == Protocol::Handshake)
            .zip(replies.iter())
        {
            chain.push((when.clone(), len.clone(), data.clone(), Some(port(&f.ports.rden_in).to_owned())));
        }
        // Each step's grant: its condition, the buffer free, nothing above it.
        let mut above = Vec::<String>::new();
        let mut grants = Vec::new();
        for (i, (when, ..)) in chain.iter().enumerate() {
            let grant = format!("reply_{i}");
            let blocked = if above.is_empty() { String::new() } else { format!(" && !({})", above.join(" || ")) };
            self.line(&format!("wire {grant} = ({when}) && !reply_full{blocked};"));
            above.push(when.clone());
            grants.push(grant);
        }
        for ((_, _, _, rden), grant) in chain.iter().zip(&grants) {
            if let Some(rden) = rden {
                self.line(&format!("assign {rden} = {grant};"));
            }
        }
        let grant_of = |when: &str| &grants[chain.iter().position(|c| c.0 == when).expect("in the chain")];
        takes.push(grant_of("hello").clone());
        takes.push(grant_of("refused").clone());
        self.line(&format!("wire rx_take = {};", takes.join(" || ")));

        // Calls out to the host, one buffer: pulses (dropped when it is
        // full), then FIFO callbacks.
        let mut calls: Vec<(String, String, String, Option<String>)> = Vec::new();
        for (when, len, data) in &pulses {
            if let Some(w) = when.strip_suffix(" /* -> call */") {
                calls.push((w.to_owned(), len.clone(), data.clone(), None));
            }
        }
        calls.append(&mut self.pending_calls);
        let mut above = Vec::<String>::new();
        let mut call_grants = Vec::new();
        for (i, (when, ..)) in calls.iter().enumerate() {
            let grant = format!("call_{i}");
            let blocked = if above.is_empty() { String::new() } else { format!(" && !({})", above.join(" || ")) };
            self.line(&format!("wire {grant} = ({when}) && !call_full{blocked};"));
            above.push(when.clone());
            call_grants.push(grant);
        }
        for ((_, _, _, rden), grant) in calls.iter().zip(&call_grants) {
            if let Some(rden) = rden {
                self.line(&format!("assign {rden} = {grant};"));
            }
        }

        // The transmitter: a call first, so the host sees a callback the
        // design made before the reply of the method that made it.
        self.line("wire send_call = !link_tx_busy && call_full;");
        self.line("wire send_reply = !link_tx_busy && !call_full && reply_full;");
        self.line("wire tx_start = send_reply || send_call;");
        self.line("wire [7:0] tx_len = send_reply ? reply_len : call_len;");
        self.line(&format!("wire [{}:0] tx_data = send_reply ? reply_data : call_data;", txw - 1));

        // The buffers load by AND-OR rather than a chain of ifs: the grants
        // exclude each other by construction, and a priority chain would be
        // a mux a level for every bit of the frame.
        for (buffer, steps, grants) in [("reply", &chain, &grants), ("call", &calls, &call_grants)] {
            if steps.is_empty() {
                self.line(&format!("wire {buffer}_load = 1'b0;\n    wire [7:0] {buffer}_next_len = '0;\n    wire [{}:0] {buffer}_next_data = '0;", txw - 1));
                continue;
            }
            let load: Vec<&str> = grants.iter().map(String::as_str).collect();
            let len: Vec<String> = steps.iter().zip(grants.iter()).map(|((_, l, ..), g)| format!("({{8{{{g}}}}} & {l})")).collect();
            let data: Vec<String> = steps.iter().zip(grants.iter()).map(|((_, _, d, _), g)| format!("({{{txw}{{{g}}}}} & {d})")).collect();
            self.line(&format!("wire {buffer}_load = {};", load.join(" | ")));
            self.line(&format!("wire [7:0] {buffer}_next_len =\n        {};", len.join("\n        | ")));
            self.line(&format!("wire [{}:0] {buffer}_next_data =\n        {};", txw - 1, data.join("\n        | ")));
        }
        let lost_grant = grants[chain.iter().position(|c| c.0 == "lost").expect("in the chain")].clone();
        self.line(&format!(
            "always_ff @(posedge clk) begin\n        if (rst) begin\n            reply_full <= 1'b0;\n            call_full <= 1'b0;\n            lost <= 1'b0;\n        end\n        else begin\n            if (send_reply) reply_full <= 1'b0;\n            if (send_call) call_full <= 1'b0;\n            if (link_rx_lost) lost <= 1'b1;\n            else if ({lost_grant}) lost <= 1'b0;\n            if (reply_load) begin reply_full <= 1'b1; reply_len <= reply_next_len; reply_data <= reply_next_data; end\n            if (call_load) begin call_full <= 1'b1; call_len <= call_next_len; call_data <= call_next_data; end\n        end\n    end"
        ));
    }

    fn call_out(&mut self, f: &Function, when: &str, len: &str, data: &str) {
        self.pending_calls.push((when.to_owned(), len.to_owned(), data.to_owned(), Some(port(&f.ports.rden_in).to_owned())));
    }
}

/// A frame's bytes as one vector: the op in byte 0, then each value in its
/// own whole bytes, little-endian, in order.
fn frame_data(op: u32, values: Vec<(String, u32, u32)>, total: u32) -> String {
    let mut parts: Vec<String> = Vec::new();
    for (expr, width, bytes) in values.iter().rev() {
        let pad = 8 * bytes - width;
        if pad > 0 {
            parts.push(format!("{pad}'d0"));
        }
        parts.push(expr.clone());
    }
    parts.push(format!("8'h{op:02x}"));
    format!("{total}'({{{}}})", parts.join(", "))
}

fn port(p: &Option<String>) -> &str {
    p.as_deref().expect("the protocol has the port")
}

/// Every port a function has, with its width, for the declarations.
fn ports_of(f: &Function) -> Vec<(String, u32)> {
    let mut out = Vec::new();
    let one = |p: &Option<String>, out: &mut Vec<(String, u32)>| {
        if let Some(p) = p {
            out.push((p.clone(), 1));
        }
    };
    one(&f.ports.valid_in, &mut out);
    one(&f.ports.rdy_out, &mut out);
    one(&f.ports.rden_in, &mut out);
    one(&f.ports.empty_out, &mut out);
    one(&f.ports.valid_out, &mut out);
    for (p, a) in f.ports.args.iter().zip(&f.args) {
        out.push((p.clone(), a.width));
    }
    if let (Some(p), Some(r)) = (&f.ports.result, &f.result) {
        out.push((p.clone(), r.width));
    }
    out
}

/// The ports the core drives into the design, for a function tied off.
fn inputs_of(f: &Function) -> Vec<(String, u32)> {
    let method = !f.callback;
    ports_of(f)
        .into_iter()
        .filter(|(p, _)| {
            let is_in = p.ends_with("_in");
            if method { is_in } else { is_in && !f.ports.args.contains(p) }
        })
        .collect()
}

fn widen(expr: &str, from: u32, to: u32) -> String {
    if from == to { expr.to_owned() } else { format!("{to}'({expr})") }
}

fn mismatch(f: &Function, contract: &str) -> String {
    let what = if !f.callback { "method" } else { "callback" };
    let args: Vec<String> = f.args.iter().map(|a| a.ty.clone()).collect();
    let result = f.result.as_ref().map_or("void".to_owned(), |r| r.ty.clone());
    let protocol = match f.latency {
        Some(n) => format!("[[latency({n})]]"),
        None => f.protocol.as_str().replace('_', " "),
    };
    format!("the {what} {}({}) -> {result} ({protocol}) cannot serve it: {contract}", f.name, args.join(", "))
}

pub mod bridge {
    //! The host bridge's plan: which functions it serves and how big its
    //! frames are. The protocol (`platform/python/kgbridge.py` speaks it):
    //!
    //! - Frames are SLIP-delimited (`0xC0` ends one; `0xDB 0xDC` and
    //!   `0xDB 0xDD` escape `0xC0` and `0xDB`).
    //! - Host to board: `[i][args]` calls method `i` (< 0x80); `[0x80|j]
    //!   [result]` answers callback `j`; `[0xFF]` asks hello.
    //! - Board to host: `[i][result]` when method `i` completes (no bytes
    //!   for `void`); `[0x80|j][args]` calls callback `j` on the host;
    //!   `[0xFF][version][hash, 4 bytes]` answers hello; `[0xFE][code][op]`
    //!   refuses a frame: 1 an unknown op, 2 a wrong length, 3 a frame lost.
    //! - Each value takes whole bytes, little-endian, in argument order.

    use super::*;

    pub const VERSION: u32 = 1;

    #[derive(Debug, Clone)]
    pub struct Plan {
        pub resource: String,
        pub baud: u32,
        /// FNV-1a over the interface and the served functions: the host
        /// checks it against its own module's.
        pub hash: u32,
        pub functions: Vec<String>,
        pub rx_bytes: u32,
        pub tx_bytes: u32,
    }

    pub fn plan(interface: &Interface, bridged: &[&Function], resource: &str, baud: u32) -> Result<Plan> {
        let functions: Vec<String> = bridged.iter().map(|f| f.name.clone()).collect();
        let bytes = |v: &[Value]| v.iter().map(Value::bytes).sum::<u32>();
        let mut rx = 1u32;
        let mut tx = 6u32; // the hello
        for f in bridged {
            let args = bytes(&f.args);
            let result = f.result.as_ref().map_or(0, Value::bytes);
            if !f.callback {
                rx = rx.max(1 + args);
                tx = tx.max(1 + result);
            } else {
                rx = rx.max(1 + result);
                tx = tx.max(1 + args);
            }
        }
        if rx > 255 || tx > 255 {
            return Err(Failure::Usage(format!("the bridge carries frames of up to 255 bytes; {} needs {}", interface.module, rx.max(tx))));
        }
        Ok(Plan { resource: resource.to_owned(), baud, hash: hash(interface, &functions), functions, rx_bytes: rx, tx_bytes: tx })
    }

    pub fn hash(interface: &Interface, functions: &[String]) -> u32 {
        let mut h: u32 = 0x811c_9dc5;
        for byte in interface.to_json().bytes().chain(functions.join(",").bytes()) {
            h ^= u32::from(byte);
            h = h.wrapping_mul(0x0100_0193);
        }
        h
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::interface::Ports;

    fn value(name: &str, width: u32) -> Value {
        Value { name: name.to_owned(), ty: format!("uint{width}"), width, signed: false, fields: Vec::new() }
    }

    /// A function with the ports its protocol gives it (`interface.rs`).
    fn function(name: &str, callback: bool, protocol: Protocol, latency: Option<u32>, args: &[u32], result: Option<u32>) -> Function {
        let (into, out_of) = if callback { ("_out", "_in") } else { ("_in", "_out") };
        let port = |role: &str| Some(format!("{name}_{role}"));
        let mut ports = Ports { args: (0..args.len()).map(|i| format!("{name}_a{i}{into}")).collect(), ..Ports::default() };
        match (callback, protocol) {
            (false, Protocol::Handshake) => {
                (ports.valid_in, ports.rdy_out, ports.rden_in, ports.empty_out) = (port("valid_in"), port("rdy_out"), port("rden_in"), port("empty_out"));
            }
            (false, Protocol::NoBackpressure) => (ports.valid_in, ports.valid_out) = (port("valid_in"), port("valid_out")),
            (false, _) => ports.valid_in = port("valid_in"),
            (true, Protocol::Handshake) => {
                (ports.rden_in, ports.empty_out, ports.valid_in, ports.rdy_out) = (port("rden_in"), port("empty_out"), port("valid_in"), port("rdy_out"));
            }
            (true, Protocol::Async) => (ports.rden_in, ports.empty_out) = (port("rden_in"), port("empty_out")),
            (true, _) => ports.valid_out = port("valid_out"),
        }
        if result.is_some() {
            ports.result = port(&format!("result{out_of}"));
        }
        Function {
            name: name.to_owned(),
            callback,
            index: 0,
            doc: None,
            protocol,
            latency,
            args: args.iter().enumerate().map(|(i, w)| value(&format!("a{i}"), *w)).collect(),
            result: result.map(|w| value("", w)),
            ports,
        }
    }

    fn interface(functions: Vec<Function>) -> Interface {
        let (mut methods, mut callbacks): (Vec<Function>, Vec<Function>) = functions.into_iter().partition(|f| !f.callback);
        for (i, f) in methods.iter_mut().enumerate() {
            f.index = i;
        }
        for (i, f) in callbacks.iter_mut().enumerate() {
            f.index = i;
        }
        Interface { version: crate::interface::VERSION, module: "Design".to_owned(), doc: None, methods, callbacks }
    }

    const RESOURCES: &str = r#"
[fpga]
device = "ice40-up5k"
part = "up5k"
hal = "ice40"
tool = "icestorm"

[board]
core-module = "test_core"

[clock]
mhz = 24

[resources.led]
kind = "led"
width = 3
pins = { out = "rgb" }

[resources.uart]
kind = "uart"
baud = 115200
pins = { rx = "uart_rxd", tx = "uart_txd" }

[resources.spi]
kind = "spi"
hz = 1000000

[resources.vga]
kind = "vga"
"#;

    fn board(connect: &[(&str, &str)]) -> Board {
        let spec: kg_config::BoardSpec = toml::from_str(RESOURCES).unwrap();
        let project = kg_config::BoardTable {
            connect: connect.iter().map(|(k, v)| ((*k).to_owned(), (*v).to_owned())).collect(),
            ..Default::default()
        };
        Board { dir: "/b".into(), platform: "/p".into(), spec, project }
    }

    /// The monitor's three functions, as `connect` names them.
    fn monitor() -> Interface {
        interface(vec![
            function("rx", false, Protocol::NoBackpressure, None, &[8], None),
            function("tx_next", false, Protocol::Fixed, Some(0), &[], Some(9)),
            function("leds", false, Protocol::Fixed, Some(0), &[], Some(3)),
        ])
    }

    #[test]
    fn functions_bind_by_connect_by_name_or_to_the_bridge() {
        let b = board(&[("rx", "uart.rx"), ("tx_next", "uart.tx"), ("leds", "led")]);
        let targets: Vec<String> = bind(&b, &monitor()).unwrap().iter().map(|(f, t)| format!("{f}={t}")).collect();
        assert_eq!(targets, ["rx=uart.rx", "tx_next=uart.tx", "leds=led"]);

        let named = interface(vec![
            function("led", false, Protocol::Fixed, Some(0), &[], Some(3)),
            function("uart_rx", false, Protocol::NoBackpressure, None, &[8], None),
            function("_uart_tx", true, Protocol::Handshake, None, &[8], None),
            function("vga", false, Protocol::Fixed, Some(1), &[10, 10], Some(12)),
            function("add", false, Protocol::Handshake, None, &[8, 8], Some(8)),
        ]);
        let targets: Vec<String> = bind(&board(&[]), &named).unwrap().iter().map(|(_, t)| t.to_string()).collect();
        assert_eq!(targets, ["led", "uart.rx", "vga.pixel", "bridge", "uart.tx"]);  // methods, then callbacks
    }

    #[test]
    fn a_binding_that_cannot_be_is_refused_by_name() {
        let err = bind(&board(&[("nope", "led")]), &monitor()).unwrap_err();
        assert!(err.message().contains("`nope`, which Design does not export"), "{}", err.message());
        let err = bind(&board(&[("rx", "uart")]), &monitor()).unwrap_err();
        assert!(err.message().contains("a uart has roles rx, tx; name one"), "{}", err.message());
        let err = bind(&board(&[("rx", "lcd")]), &monitor()).unwrap_err();
        assert!(err.message().contains("no resource `lcd`"), "{}", err.message());
        let err = bind(&board(&[("rx", "led"), ("leds", "led")]), &monitor()).unwrap_err();
        assert!(err.message().contains("rx and leds both bind to led"), "{}", err.message());
    }

    #[test]
    fn the_monitors_core_is_wired_as_its_hand_written_one_was() {
        let core = generate(&board(&[("rx", "uart.rx"), ("tx_next", "uart.tx"), ("leds", "led")]), &monitor()).unwrap();
        for line in [
            "module test_core",
            "assign rx_valid_in = uart_rx_valid & started;",
            "assign tx_next_valid_in = uart_tx_idle & started;",
            "assign uart_tx_start = uart_tx_idle & started & tx_next_result_out[8];",
            "uart_rx #(.CLKS_PER_BIT(208)) uart_receiver",
            "assign leds_valid_in = started;",
            "assign rgb = leds_result_out;",
            // Resources nothing uses sit idle.
            "assign spi_cs_n = 1'b1;",
            "assign vga_hsync = 1'b1;",
        ] {
            assert!(core.text.contains(line), "missing `{line}` in\n{}", core.text);
        }
        assert!(core.bridge.is_none());
    }

    /// A shape that fits no contract is refused with the contract spelled
    /// out, never wired approximately.
    #[test]
    fn a_function_that_fits_no_contract_says_what_would() {
        let wide = interface(vec![function("leds", false, Protocol::Fixed, Some(0), &[], Some(9))]);
        let err = generate(&board(&[("leds", "led")]), &wide).unwrap_err();
        assert!(err.message().contains("the method leds() -> uint9 ([[latency(0)]]) cannot serve it"), "{}", err.message());
        assert!(err.message().contains("returning up to uint3"), "{}", err.message());
        let blocking = interface(vec![function("rx", false, Protocol::Handshake, None, &[16], None)]);
        let err = generate(&board(&[("rx", "uart.rx")]), &blocking).unwrap_err();
        assert!(err.message().contains("takes a method taking one uint8"), "{}", err.message());
        let spi = interface(vec![function("_spi", true, Protocol::Handshake, None, &[8], Some(8))]);
        let err = generate(&board(&[]), &spi).unwrap_err();
        assert!(err.message().contains("(uint8 mosi, bool hold) -> uint8"), "{}", err.message());
    }

    #[test]
    fn unbound_functions_get_the_bridge_unless_the_port_is_taken() {
        let accel = interface(vec![
            function("add", false, Protocol::Handshake, None, &[32, 32], Some(32)),
            function("_square", true, Protocol::Handshake, None, &[16], Some(32)),
        ]);
        let core = generate(&board(&[]), &accel).unwrap();
        let plan = core.bridge.as_ref().unwrap();
        assert_eq!((plan.rx_bytes, plan.tx_bytes, plan.baud), (9, 6, 115_200));
        assert_eq!(plan.functions, ["add", "_square"]);
        assert!(core.text.contains("assign add_a0_in = link_rx_data[8 +: 32];"), "{}", core.text);
        assert!(core.text.contains("assign add_a1_in = link_rx_data[40 +: 32];"), "{}", core.text);
        assert!(core.text.contains("wire send_call = !link_tx_busy && call_full;"), "{}", core.text);
        assert_eq!(plan.hash, bridge::hash(&accel, &plan.functions));

        let mut mixed = monitor();
        mixed.methods.push(function("add", false, Protocol::Handshake, None, &[8], Some(8)));
        let err = generate(&board(&[("rx", "uart.rx"), ("tx_next", "uart.tx"), ("leds", "led")]), &mixed).unwrap_err();
        assert!(err.message().contains("the bridge needs the serial port `uart`"), "{}", err.message());
    }
}
