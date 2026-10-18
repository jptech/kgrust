//! The interface description: an exported class as seen from outside the
//! generated module (`docs/plans/board-tooling.md` §4). Every `kg board`
//! build writes it to `target/board/interface.json`; the wrapper generator,
//! the host bridge, the Python module and the cocotb driver all read it, so
//! the four agree on every port by construction.
//!
//! It joins two sources and checks each against the other:
//! - the class's declaration, read from the project's sources: names,
//!   argument order, signedness, attributes and documentation;
//! - the generated module's own port list: exact names, widths, directions
//!   and the protocol each function has.
//!
//! The protocols are the backend's (`compiler/cpp/verilog.cpp`):
//! - a method (`GetExportInterfaces`, `verilog.cpp:6465-6539`) takes
//!   `<m>_valid_in` and its arguments as `<m>_<arg>_in`. With backpressure it
//!   answers `<m>_rdy_out`, and returns through a show-ahead FIFO,
//!   `<m>_empty_out`, `<m>_rden_in`, `<m>_result_out`. With
//!   `[[no_backpressure]]` the FIFO becomes a pulse, `<m>_valid_out`. With
//!   `[[latency(N)]]` the result is simply there N cycles later.
//! - a callback (`DeclareCorePorts`, `verilog.cpp:6582-6662`) is the same
//!   from the other side. Its arguments queue in a FIFO, `<cb>_empty_out`,
//!   `<cb>_rden_in`, `<cb>_<arg>_out`, and its result is pushed back with
//!   `<cb>_valid_in` and `<cb>_result_in` while `<cb>_rdy_out`. A
//!   `[[no_backpressure]]` callback is a pulse, `<cb>_valid_out`, and a
//!   `[[latency(N)]]` one (`WriteExternalModuleCallPorts`,
//!   `verilog.cpp:7529-7556`) a pulse whose result is due N cycles later.
//!
//! Every port is claimed by exactly one function or by the module's fixed
//! ports; a port nobody claims, or a function whose ports are missing, is an
//! error rather than a wrong wrapper.

use std::collections::HashMap;

use camino::{Utf8Path, Utf8PathBuf};
use kg_syntax::{NodeOrToken, SyntaxKind, SyntaxKind::*, SyntaxNode};
use serde::{Deserialize, Serialize};

use crate::{Failure, Result};

/// The description's format; a reader refuses another.
pub const VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Interface {
    pub version: u32,
    /// The generated module, which is the exported class's name.
    pub module: String,
    pub doc: Option<String>,
    /// Public methods, in declaration order; `index` is the place here.
    pub methods: Vec<Function>,
    /// Callbacks, in declaration order, indexed apart from the methods.
    pub callbacks: Vec<Function>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Function {
    pub name: String,
    /// A callback, which the design calls out through; else a method.
    pub callback: bool,
    pub index: usize,
    pub doc: Option<String>,
    pub protocol: Protocol,
    /// `[[latency(N)]]`'s N, for [`Protocol::Fixed`].
    pub latency: Option<u32>,
    pub args: Vec<Value>,
    /// `None` for `void`.
    pub result: Option<Value>,
    pub ports: Ports,
}

/// How a function is called, as its ports say.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Protocol {
    /// Ready/valid in, a show-ahead FIFO out (a callback: the reverse).
    Handshake,
    /// Valid in, never refused; a pulse out when it completes.
    NoBackpressure,
    /// Valid in; the result N cycles later, no handshake at all.
    Fixed,
    /// A callback queued with no answer: `[[async]]`.
    Async,
}

impl Protocol {
    pub fn as_str(self) -> &'static str {
        match self {
            Protocol::Handshake => "handshake",
            Protocol::NoBackpressure => "no_backpressure",
            Protocol::Fixed => "fixed",
            Protocol::Async => "async",
        }
    }
}

/// An argument or a result: its bits, and how to read them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Value {
    /// The argument's name; empty for a result.
    pub name: String,
    /// The type as declared, `uint8`, `Pair`, `uint<W>`.
    #[serde(rename = "type")]
    pub ty: String,
    pub width: u32,
    pub signed: bool,
    /// A packed struct's fields, least significant first: the backend packs
    /// the first declared field lowest.
    pub fields: Vec<Value>,
}

/// The generated module's port for each role a function has; absent roles
/// are `None`. `args` is in argument order.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ports {
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub valid_in: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub rdy_out: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub rden_in: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub empty_out: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub valid_out: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub result: Option<String>,
    pub args: Vec<String>,
}

impl Interface {
    pub fn method(&self, name: &str) -> Option<&Function> {
        self.methods.iter().find(|f| f.name == name)
    }

    pub fn callback(&self, name: &str) -> Option<&Function> {
        self.callbacks.iter().find(|f| f.name == name)
    }

    /// Any function, method or callback, by name.
    pub fn function(&self, name: &str) -> Option<&Function> {
        self.method(name).or_else(|| self.callback(name))
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).expect("an interface serialises") + "\n"
    }

    pub fn from_json(text: &str) -> std::result::Result<Interface, String> {
        let interface: Interface = serde_json::from_str(text).map_err(|e| e.to_string())?;
        if interface.version != VERSION {
            return Err(format!("interface version {}, this kg reads {VERSION}", interface.version));
        }
        Ok(interface)
    }

    /// For `kg board interface`: a line per function, its protocol and ports.
    pub fn summary(&self) -> String {
        let mut text = format!("{} ({} methods, {} callbacks)\n", self.module, self.methods.len(), self.callbacks.len());
        for (title, list) in [("methods", &self.methods), ("callbacks", &self.callbacks)] {
            if list.is_empty() {
                continue;
            }
            text.push_str(&format!("  {title}:\n"));
            for f in list {
                let args: Vec<String> = f.args.iter().map(|a| format!("{} {}", a.ty, a.name)).collect();
                let result = f.result.as_ref().map_or("void".to_owned(), |r| r.ty.clone());
                let protocol = match f.latency {
                    Some(n) => format!("latency {n}"),
                    None => f.protocol.as_str().replace('_', " "),
                };
                text.push_str(&format!("    {}({}) -> {result}  [{protocol}]\n", f.name, args.join(", ")));
                if let Some(doc) = &f.doc {
                    for line in doc.lines().filter(|l| !l.trim().is_empty()).take(2) {
                        text.push_str(&format!("        {}\n", line.trim()));
                    }
                }
            }
        }
        text
    }
}

impl Function {
    /// `name(uint10 x, uint10 y) -> uint12`, as the front panel shows it.
    pub fn signature(&self) -> String {
        let args: Vec<String> = self.args.iter().map(|a| format!("{} {}", a.ty, a.name)).collect();
        let result = self.result.as_ref().map_or("void".to_owned(), |r| r.ty.clone());
        format!("{}({}) -> {result}", self.name, args.join(", "))
    }
}

impl Value {
    /// A struct's field by name.
    pub fn field(&self, name: &str) -> Option<&Value> {
        self.fields.iter().find(|f| f.name == name)
    }

    /// The bytes the value takes whole, as the bridge carries it.
    pub fn bytes(&self) -> u32 {
        self.width.div_ceil(8)
    }
}

/// Describes `module` from its generated RTL (`module_sv`, the text of
/// `<module>.sv`) and the project's sources.
pub fn describe(module: &str, module_sv: &str, sources: &[(Utf8PathBuf, String)]) -> Result<Interface> {
    let ports = ports(module_sv, module).map_err(Failure::Failed)?;
    let declared = declared(module, sources)?;
    join(module, &ports, &declared).map_err(|e| Failure::Failed(format!("interface of {module}: {e}")))
}

/// The generated module's name in a build directory: the one `<M>.sv` with
/// a `<M>_types.sv` beside it.
pub fn module_name(generated: &[Utf8PathBuf]) -> Option<String> {
    generated.iter().find_map(|p| p.file_name()?.strip_suffix("_types.sv").map(str::to_owned))
}

// ---------------------------------------------------------------------------
// The generated module's ports.

/// One port of the generated module.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Port {
    pub name: String,
    pub input: bool,
    pub shape: Shape,
}

/// A port's type: its width, and a packed struct's fields as written (most
/// significant first).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shape {
    pub width: u32,
    pub fields: Vec<(String, Shape)>,
}

/// The port list of `module` in CIRCT's output: `module M(` then a port a
/// line, `input  wire [7:0] name,`, up to `);`.
pub fn ports(text: &str, module: &str) -> std::result::Result<Vec<Port>, String> {
    let header = format!("module {module}(");
    let start = text
        .lines()
        .position(|l| l.trim_end() == header || l.starts_with(&format!("{header}\t")) || l.starts_with(&format!("{header} ")))
        .ok_or_else(|| format!("no `{header}` in the generated RTL"))?;
    let mut ports = Vec::new();
    for line in text.lines().skip(start + 1) {
        // A location comment may follow the port.
        let line = line.split("//").next().unwrap_or("").trim();
        if line.starts_with(");") || line == ")" {
            return Ok(ports);
        }
        let line = line.trim_end_matches(',').trim();
        if line.is_empty() {
            continue;
        }
        ports.push(port(line).ok_or_else(|| format!("cannot read the port `{line}` of {module}"))?);
    }
    Err(format!("the port list of {module} never closes"))
}

fn port(line: &str) -> Option<Port> {
    let (direction, rest) = line.split_once(char::is_whitespace)?;
    let input = match direction {
        "input" => true,
        "output" => false,
        _ => return None,
    };
    let rest = rest.trim_start();
    let rest = rest.strip_prefix("wire").or_else(|| rest.strip_prefix("logic")).unwrap_or(rest).trim();
    let split = rest.rfind(|c: char| c.is_whitespace() || c == '}' || c == ']').map_or(0, |i| i + 1);
    let (ty, name) = (rest[..split].trim(), rest[split..].trim());
    if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return None;
    }
    Some(Port { name: name.to_owned(), input, shape: shape(ty)? })
}

/// `""`, `[7:0]`, `[3:0][7:0]`, `logic [7:0]`, `struct packed {…}`.
fn shape(ty: &str) -> Option<Shape> {
    let ty = ty.trim();
    let ty = ty.strip_prefix("logic").map(str::trim_start).unwrap_or(ty);
    let ty = ty.strip_prefix("unsigned").or_else(|| ty.strip_prefix("signed")).map(str::trim_start).unwrap_or(ty);
    if let Some(body) = ty.strip_prefix("struct packed") {
        let body = body.trim();
        let close = matching_brace(body)?;
        let inner = &body[1..close];
        let mut fields = Vec::new();
        for member in split_members(inner) {
            let member = member.trim();
            if member.is_empty() {
                continue;
            }
            let split = member.rfind(|c: char| c.is_whitespace() || c == '}' || c == ']')? + 1;
            let field = shape(&member[..split])?;
            fields.push((member[split..].trim().to_owned(), field));
        }
        let width = fields.iter().map(|(_, s)| s.width).sum::<u32>() * dims(&body[close + 1..])?;
        return Some(Shape { width, fields });
    }
    Some(Shape { width: dims(ty)?, fields: Vec::new() })
}

/// The product of `[H:L]` ranges; 1 for none.
fn dims(text: &str) -> Option<u32> {
    let mut width = 1u32;
    let mut rest = text.trim();
    while let Some(open) = rest.strip_prefix('[') {
        let close = open.find(']')?;
        let (hi, lo) = open[..close].split_once(':')?;
        let (hi, lo): (i64, i64) = (hi.trim().parse().ok()?, lo.trim().parse().ok()?);
        width = width.checked_mul(u32::try_from((hi - lo).abs() + 1).ok()?)?;
        rest = open[close + 1..].trim_start();
    }
    rest.is_empty().then_some(width)
}

fn matching_brace(text: &str) -> Option<usize> {
    let mut depth = 0;
    for (i, c) in text.char_indices() {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
    }
    None
}

/// `a; b; struct packed {c; d;} e;` split on the top level's semicolons.
fn split_members(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let (mut depth, mut start) = (0, 0);
    for (i, c) in text.char_indices() {
        match c {
            '{' => depth += 1,
            '}' => depth -= 1,
            ';' if depth == 0 => {
                out.push(&text[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    out.push(&text[start..]);
    out
}

// ---------------------------------------------------------------------------
// The class's declaration.

/// The exported class as declared.
#[derive(Debug, Default)]
struct Declared {
    doc: Option<String>,
    methods: Vec<DeclaredFn>,
    callbacks: Vec<DeclaredFn>,
    /// Every struct the sources declare, by name: its fields' names and types.
    structs: HashMap<String, Vec<(String, DeclaredType)>>,
    /// Every `using A = B;`.
    aliases: HashMap<String, DeclaredType>,
}

#[derive(Debug)]
struct DeclaredFn {
    name: String,
    doc: Option<String>,
    params: Vec<(String, DeclaredType)>,
    result: Option<DeclaredType>,
    latency: Option<u32>,
    no_backpressure: bool,
    reset: bool,
}

/// A type as written: its text, and whether it reads as signed (`intN`,
/// `int<W>`), a named type to look up, or neither.
#[derive(Debug, Clone)]
struct DeclaredType {
    text: String,
    sign: Sign,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Sign {
    Signed,
    Unsigned,
    Named(String),
}

fn declared(class: &str, sources: &[(Utf8PathBuf, String)]) -> Result<Declared> {
    let mut found = None;
    let mut decl = Declared::default();
    for (path, text) in sources {
        let root = kg_syntax::parse_file(text).syntax_node();
        let docs = kg_syntax::file_docs(&root);
        for node in root.descendants() {
            match node.kind() {
                STRUCT_DECL => {
                    if let Some(name) = name_of(&node) {
                        decl.structs.insert(name, struct_fields(&node));
                    }
                }
                ALIAS_DECL => {
                    if let (Some(name), Some(ty)) = (name_of(&node), node.children().find(|n| is_type(n.kind()))) {
                        decl.aliases.insert(name, declared_type(&ty));
                    }
                }
                CLASS_DECL if name_of(&node).as_deref() == Some(class) && found.is_none() => {
                    let doc = name_token(&node).and_then(|t| docs.around(t.text_range())).map(kg_syntax::Doc::text);
                    let (methods, callbacks) = members(&node, &docs);
                    found = Some(path.clone());
                    decl.doc = doc;
                    decl.methods = methods;
                    decl.callbacks = callbacks;
                }
                _ => {}
            }
        }
    }
    if found.is_none() {
        let files: Vec<&str> = sources.iter().map(|(p, _)| p.file_name().unwrap_or(p.as_str())).collect();
        return Err(Failure::Failed(format!(
            "interface: the class {class} is not declared in the project's sources ({}); the description reads its declaration there",
            files.join(", ")
        )));
    }
    Ok(decl)
}

fn members(class: &SyntaxNode, docs: &kg_syntax::FileDocs) -> (Vec<DeclaredFn>, Vec<DeclaredFn>) {
    let (mut methods, mut callbacks) = (Vec::new(), Vec::new());
    let Some(body) = class.children().find(|n| n.kind() == DECL_LIST) else { return (methods, callbacks) };
    let mut public = false;
    for member in body.children() {
        let doc = || name_token(&member).and_then(|t| docs.around(t.text_range())).map(kg_syntax::Doc::text);
        match member.kind() {
            VISIBILITY => public = member.text().to_string().trim_start().starts_with("public"),
            FN_DECL if public => {
                let Some(name) = name_of(&member) else { continue };
                let params = member.children().find(|n| n.kind() == PARAM_LIST).map(|l| params(&l, PARAM)).unwrap_or_default();
                let result = member.children().find(|n| is_type(n.kind())).filter(|t| t.kind() != VOID_TYPE).map(|t| declared_type(&t));
                let attrs = attributes(&member);
                methods.push(DeclaredFn { name, doc: doc(), params, result, latency: attrs.latency, no_backpressure: attrs.no_backpressure, reset: attrs.reset });
            }
            VAR_DECL => {
                let Some(fn_type) = member.children().find(|n| n.kind() == FN_TYPE) else { continue };
                let Some(name) = name_of(&member) else { continue };
                let params = fn_type.children().find(|n| n.kind() == FN_TYPE_PARAM_LIST).map(|l| params(&l, FN_TYPE_PARAM)).unwrap_or_default();
                let result = fn_type.children().filter(|n| is_type(n.kind())).last().filter(|t| t.kind() != VOID_TYPE).map(|t| declared_type(&t));
                let attrs = attributes(&member);
                callbacks.push(DeclaredFn { name, doc: doc(), params, result, latency: attrs.latency, no_backpressure: attrs.no_backpressure, reset: false });
            }
            _ => {}
        }
    }
    (methods, callbacks)
}

#[derive(Default)]
struct Attrs {
    latency: Option<u32>,
    no_backpressure: bool,
    reset: bool,
}

fn attributes(node: &SyntaxNode) -> Attrs {
    let mut attrs = Attrs::default();
    for attr in node.children().filter(|n| n.kind() == ATTR_LIST).flat_map(|l| l.children()).filter(|n| n.kind() == ATTR) {
        let name = attr.first_token().map(|t| t.text().to_owned()).unwrap_or_default();
        let arg = attr.children().find(|n| n.kind() == ATTR_ARGS).map(|a| compact(&a));
        match name.as_str() {
            "latency" => attrs.latency = arg.and_then(|a| a.trim_matches(|c| c == '(' || c == ')').parse().ok()),
            "no_backpressure" => attrs.no_backpressure = true,
            "reset" => attrs.reset = true,
            _ => {}
        }
    }
    attrs
}

fn params(list: &SyntaxNode, kind: SyntaxKind) -> Vec<(String, DeclaredType)> {
    list.children()
        .filter(|n| n.kind() == kind)
        .filter_map(|p| {
            let ty = p.children().find(|n| is_type(n.kind()))?;
            Some((name_of(&p).unwrap_or_default(), declared_type(&ty)))
        })
        .collect()
}

fn struct_fields(node: &SyntaxNode) -> Vec<(String, DeclaredType)> {
    let Some(body) = node.children().find(|n| n.kind() == DECL_LIST) else { return Vec::new() };
    body.children()
        .filter(|n| n.kind() == VAR_DECL)
        .filter_map(|f| Some((name_of(&f)?, declared_type(&f.children().find(|n| is_type(n.kind()))?))))
        .collect()
}

fn is_type(kind: SyntaxKind) -> bool {
    matches!(kind, INT_TYPE | PARAM_INT_TYPE | BOOL_TYPE | VOID_TYPE | PATH_TYPE | FLOAT_TYPE | ARRAY_TYPE)
}

fn declared_type(node: &SyntaxNode) -> DeclaredType {
    let text = compact(node);
    let first = node.descendants_with_tokens().filter_map(NodeOrToken::into_token).find(|t| !t.kind().is_trivia());
    let sign = match (node.kind(), first.as_ref().map(|t| t.kind())) {
        (INT_TYPE | PARAM_INT_TYPE, Some(INT_TYPE_TOKEN | INT_KW)) => Sign::Signed,
        (PATH_TYPE, _) => Sign::Named(text.clone()),
        _ if text.starts_with("int") => Sign::Signed,
        _ => Sign::Unsigned,
    };
    DeclaredType { text, sign }
}

fn name_token(node: &SyntaxNode) -> Option<kg_syntax::SyntaxToken> {
    let name = node.children().find(|n| n.kind() == NAME)?;
    name.descendants_with_tokens().filter_map(NodeOrToken::into_token).find(|t| !t.kind().is_trivia())
}

fn name_of(node: &SyntaxNode) -> Option<String> {
    name_token(node).map(|t| t.text().to_owned())
}

/// The node's tokens without trivia, spaced only where both sides are words.
fn compact(node: &SyntaxNode) -> String {
    let mut text = String::new();
    let mut last_word = false;
    for token in node.descendants_with_tokens().filter_map(NodeOrToken::into_token).filter(|t| !t.kind().is_trivia()) {
        let word = token.text().starts_with(|c: char| c.is_alphanumeric() || c == '_');
        if word && last_word {
            text.push(' ');
        }
        text.push_str(token.text());
        last_word = word && token.text().ends_with(|c: char| c.is_alphanumeric() || c == '_');
    }
    text
}

// ---------------------------------------------------------------------------
// The two, joined.

/// Ports every generated module has, claimed by no function.
const FIXED_PORTS: &[&str] = &["clk", "rst", "rst_and_startup_done_out", "stall_rate_in", "stall_rate_valid_in", "stall_rate_supported_out"];

fn join(module: &str, ports: &[Port], declared: &Declared) -> std::result::Result<Interface, String> {
    let by_name: HashMap<&str, &Port> = ports.iter().map(|p| (p.name.as_str(), p)).collect();
    let mut claimed: Vec<String> = FIXED_PORTS.iter().map(|p| (*p).to_owned()).collect();
    let mut methods = Vec::new();
    for decl in declared.methods.iter().filter(|m| !m.reset) {
        let f = function(decl, methods.len(), false, &by_name, declared)?;
        claim(&f.ports, &mut claimed);
        methods.push(f);
    }
    let mut callbacks = Vec::new();
    for decl in &declared.callbacks {
        let f = function(decl, callbacks.len(), true, &by_name, declared)?;
        claim(&f.ports, &mut claimed);
        callbacks.push(f);
    }
    let unclaimed: Vec<&str> = ports.iter().map(|p| p.name.as_str()).filter(|n| !claimed.iter().any(|c| c == n)).collect();
    if !unclaimed.is_empty() {
        return Err(format!("the generated module has ports no declared function explains: {}", unclaimed.join(", ")));
    }
    Ok(Interface { version: VERSION, module: module.to_owned(), doc: declared.doc.clone(), methods, callbacks })
}

fn claim(ports: &Ports, claimed: &mut Vec<String>) {
    let roles = [&ports.valid_in, &ports.rdy_out, &ports.rden_in, &ports.empty_out, &ports.valid_out, &ports.result];
    claimed.extend(roles.into_iter().flatten().cloned());
    claimed.extend(ports.args.iter().cloned());
}

fn function(
    decl: &DeclaredFn,
    index: usize,
    callback: bool,
    by_name: &HashMap<&str, &Port>,
    declared: &Declared,
) -> std::result::Result<Function, String> {
    let name = &decl.name;
    // A method's inputs are the callback's outputs, and the reverse.
    let (into, out_of) = if callback { ("_out", "_in") } else { ("_in", "_out") };
    let find = |role: &str, input: bool| -> std::result::Result<Option<String>, String> {
        let port = format!("{name}_{role}");
        match by_name.get(port.as_str()) {
            Some(p) if p.input == input => Ok(Some(port)),
            Some(_) => Err(format!("{port} has the wrong direction")),
            None => Ok(None),
        }
    };
    let mut ports = Ports {
        valid_in: find("valid_in", true)?,
        rdy_out: find("rdy_out", false)?,
        rden_in: find("rden_in", true)?,
        empty_out: find("empty_out", false)?,
        valid_out: find("valid_out", false)?,
        result: find(&format!("result{out_of}"), callback)?,
        args: Vec::new(),
    };
    let mut args = Vec::new();
    for (arg, ty) in &decl.params {
        let port = format!("{name}_{arg}{into}");
        let p = by_name.get(port.as_str()).ok_or_else(|| format!("{name}: no port {port} for the argument `{arg}`"))?;
        args.push(value(arg, ty, &p.shape, declared));
        ports.args.push(port);
    }
    let result = match (&decl.result, &ports.result) {
        (Some(ty), Some(port)) => Some(value("", ty, &by_name[port.as_str()].shape, declared)),
        (None, None) => None,
        (Some(ty), None) => {
            return Err(format!("{name} returns {} but the module has no {name}_result{out_of}", ty.text));
        }
        (None, Some(port)) => return Err(format!("{name} returns void but the module has {port}")),
    };

    let has = |p: &Option<String>| p.is_some();
    let protocol = if callback {
        match (has(&ports.rden_in) && has(&ports.empty_out), has(&ports.valid_out)) {
            (true, false) if has(&ports.valid_in) && has(&ports.rdy_out) => Protocol::Handshake,
            (true, false) if !has(&ports.valid_in) && !has(&ports.rdy_out) && result.is_none() => Protocol::Async,
            (false, true) if decl.latency.is_some() => Protocol::Fixed,
            (false, true) if decl.no_backpressure && result.is_none() && !has(&ports.valid_in) => Protocol::NoBackpressure,
            _ => return Err(format!("the callback {name}'s ports fit no protocol: {}", port_list(&ports))),
        }
    } else {
        if !has(&ports.valid_in) {
            return Err(format!("the method {name} has no port {name}_valid_in"));
        }
        if has(&ports.rdy_out) {
            if decl.no_backpressure || decl.latency.is_some() {
                return Err(format!("{name} is declared without backpressure but has {name}_rdy_out"));
            }
            if has(&ports.valid_out) || has(&ports.rden_in) != has(&ports.empty_out) {
                return Err(format!("the method {name}'s ports fit no protocol: {}", port_list(&ports)));
            }
            Protocol::Handshake
        } else if decl.latency.is_some() {
            if has(&ports.valid_out) || has(&ports.rden_in) || has(&ports.empty_out) {
                return Err(format!("{name} is [[latency]] but has a result handshake: {}", port_list(&ports)));
            }
            Protocol::Fixed
        } else if decl.no_backpressure {
            if has(&ports.rden_in) || has(&ports.empty_out) {
                return Err(format!("the method {name}'s ports fit no protocol: {}", port_list(&ports)));
            }
            Protocol::NoBackpressure
        } else {
            return Err(format!("{name} has no {name}_rdy_out, yet is declared with backpressure"));
        }
    };
    Ok(Function {
        name: name.clone(),
        callback,
        index,
        doc: decl.doc.clone(),
        protocol,
        latency: if protocol == Protocol::Fixed { decl.latency } else { None },
        args,
        result,
        ports,
    })
}

fn port_list(ports: &Ports) -> String {
    let roles = [&ports.valid_in, &ports.rdy_out, &ports.rden_in, &ports.empty_out, &ports.valid_out, &ports.result];
    let names: Vec<&str> = roles.into_iter().flatten().map(String::as_str).collect();
    if names.is_empty() { "none".to_owned() } else { names.join(", ") }
}

/// A value: the port's width and layout, the declaration's names and signs.
fn value(name: &str, ty: &DeclaredType, shape: &Shape, declared: &Declared) -> Value {
    let resolved = resolve(ty, declared, 0);
    let fields = match &resolved.sign {
        Sign::Named(struct_name) => {
            let decls = declared.structs.get(struct_name);
            // As written, most significant first; listed least first.
            shape
                .fields
                .iter()
                .rev()
                .map(|(field, s)| {
                    let field_ty = decls
                        .and_then(|d| d.iter().find(|(n, _)| n == field))
                        .map(|(_, t)| t.clone())
                        .unwrap_or(DeclaredType { text: format!("uint{}", s.width), sign: Sign::Unsigned });
                    value(field, &field_ty, s, declared)
                })
                .collect()
        }
        _ => Vec::new(),
    };
    Value { name: name.to_owned(), ty: ty.text.clone(), width: shape.width, signed: resolved.sign == Sign::Signed, fields }
}

/// Follows `using` aliases to a sign or a struct's name.
fn resolve(ty: &DeclaredType, declared: &Declared, depth: usize) -> DeclaredType {
    match &ty.sign {
        Sign::Named(n) if depth < 16 && !declared.structs.contains_key(n) => match declared.aliases.get(n) {
            Some(target) => resolve(target, declared, depth + 1),
            None => DeclaredType { text: ty.text.clone(), sign: Sign::Unsigned },
        },
        _ => ty.clone(),
    }
}

/// Reads the project's sources for [`describe`].
pub fn read_sources(paths: &[Utf8PathBuf]) -> Result<Vec<(Utf8PathBuf, String)>> {
    paths
        .iter()
        .filter(|p| p.extension() == Some("k"))
        .map(|p| std::fs::read_to_string(p).map(|t| (p.clone(), t)).map_err(|e| Failure::Failed(format!("{p}: {e}"))))
        .collect()
}

/// Writes the description where the other generators read it.
pub fn write(interface: &Interface, out: &Utf8Path) -> Result<Utf8PathBuf> {
    let path = out.join("interface.json");
    crate::write(&path, &interface.to_json())?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The probe of `docs/plans/board-tooling.md` §4: one function of every
    /// protocol, its port list as the backend wrote it (2026-10-17).
    const PROBE_K: &str = r#"
struct Pair
{
    uint8 a;
    int4 b;
}

using word = int16;

//| A probe of every protocol.
class Probe
{
private:
    uint32 _count;
    (uint8 c) -> void _tx;
    //| One SPI byte.
    (uint8 mosi, bool hold) -> uint8 _spi;
    [[latency(0)]] (uint8 c) -> void _fast;

public:
    //| Adds.
    void plain(uint8 x, word y) { _tx(x); }
    uint16 handshake(uint8 x) { return _spi(x, true) + x; }
    [[no_backpressure]] void nbp(bool b) { _fast(b ? 1 : 0); }
    [[latency(1)]] uint9 lat1(uint3 i) { return i; }
    Pair pair(Pair p) { return p; }
    int8 signed_ret(int8 v) { return -v; }
}

export Probe;
"#;

    const PROBE_SV: &str = "module Probe(
  input  wire                                                clk,
  input  wire                                                rst,
  input  wire                                                plain_valid_in,
  input  wire [7:0]                                          plain_x_in,
  input  wire [15:0]                                         plain_y_in,
  input  wire                                                plain_rden_in,
  input  wire                                                handshake_valid_in,
  input  wire [7:0]                                          handshake_x_in,
  input  wire                                                handshake_rden_in,
  input  wire                                                nbp_valid_in,
  input  wire                                                nbp_b_in,
  input  wire                                                lat1_valid_in,
  input  wire [2:0]                                          lat1_i_in,
  input  wire                                                pair_valid_in,
  input  wire struct packed {logic [3:0] b; logic [7:0] a; } pair_p_in,
  input  wire                                                pair_rden_in,
  input  wire                                                signed_ret_valid_in,
  input  wire [7:0]                                          signed_ret_v_in,
  input  wire                                                signed_ret_rden_in,
  input  wire                                                _spi_rden_in,
  input  wire                                                _spi_valid_in,
  input  wire [7:0]                                          _spi_result_in,
  input  wire                                                _tx_rden_in,
  input  wire                                                _tx_valid_in,
  input  wire [2:0]                                          stall_rate_in,
  input  wire                                                stall_rate_valid_in,
  output wire                                                rst_and_startup_done_out,
  output wire                                                plain_rdy_out,
  output wire                                                plain_empty_out,
  output wire                                                handshake_rdy_out,
  output wire                                                handshake_empty_out,
  output wire [15:0]                                         handshake_result_out,
  output wire                                                nbp_valid_out,
  output wire [8:0]                                          lat1_result_out,
  output wire                                                pair_rdy_out,
  output wire                                                pair_empty_out,
  output wire struct packed {logic [3:0] b; logic [7:0] a; } pair_result_out,
  output wire                                                signed_ret_rdy_out,
  output wire                                                signed_ret_empty_out,
  output wire [7:0]                                          signed_ret_result_out,
  output wire [7:0]                                          _spi_mosi_out,
  output wire                                                _spi_hold_out,
  output wire                                                _spi_empty_out,
  output wire                                                _spi_rdy_out,
  output wire [7:0]                                          _tx_c_out,
  output wire                                                _tx_empty_out,
  output wire                                                _tx_rdy_out,
  output wire                                                _fast_valid_out,
  output wire [7:0]                                          _fast_c_out,
  output wire                                                stall_rate_supported_out
);
";

    fn probe() -> Interface {
        describe("Probe", PROBE_SV, &[(Utf8PathBuf::from("probe.k"), PROBE_K.to_owned())]).unwrap()
    }

    #[test]
    fn ports_are_read_with_widths_and_struct_layouts() {
        let ports = ports(PROBE_SV, "Probe").unwrap();
        assert_eq!(ports.len(), 50);
        let pair = ports.iter().find(|p| p.name == "pair_p_in").unwrap();
        assert!(pair.input);
        assert_eq!(pair.shape.width, 12);
        assert_eq!(pair.shape.fields.iter().map(|(n, s)| (n.as_str(), s.width)).collect::<Vec<_>>(), [("b", 4), ("a", 8)]);
        assert_eq!(shape("[3:0][7:0]").unwrap().width, 32);
        assert_eq!(shape("").unwrap().width, 1);
    }

    #[test]
    fn every_method_protocol_is_read_off_its_ports() {
        let probe = probe();
        let protocols: Vec<(&str, Protocol, Option<u32>)> =
            probe.methods.iter().map(|f| (f.name.as_str(), f.protocol, f.latency)).collect();
        assert_eq!(
            protocols,
            [
                ("plain", Protocol::Handshake, None),
                ("handshake", Protocol::Handshake, None),
                ("nbp", Protocol::NoBackpressure, None),
                ("lat1", Protocol::Fixed, Some(1)),
                ("pair", Protocol::Handshake, None),
                ("signed_ret", Protocol::Handshake, None),
            ]
        );
        let plain = probe.method("plain").unwrap();
        assert_eq!(plain.ports.rden_in.as_deref(), Some("plain_rden_in"));
        assert_eq!(plain.ports.args, ["plain_x_in", "plain_y_in"]);
        assert!(plain.result.is_none());
        assert_eq!(probe.method("nbp").unwrap().ports.valid_out.as_deref(), Some("nbp_valid_out"));
        assert_eq!(probe.method("lat1").unwrap().ports.result.as_deref(), Some("lat1_result_out"));
    }

    #[test]
    fn every_callback_protocol_is_read_off_its_ports() {
        let probe = probe();
        let protocols: Vec<(&str, Protocol, usize)> =
            probe.callbacks.iter().map(|f| (f.name.as_str(), f.protocol, f.index)).collect();
        assert_eq!(protocols, [("_tx", Protocol::Handshake, 0), ("_spi", Protocol::Handshake, 1), ("_fast", Protocol::Fixed, 2)]);
        let spi = probe.callback("_spi").unwrap();
        assert_eq!(spi.ports.args, ["_spi_mosi_out", "_spi_hold_out"]);
        assert_eq!(spi.ports.result.as_deref(), Some("_spi_result_in"));
        assert_eq!(spi.result.as_ref().unwrap().width, 8);
        assert_eq!(spi.doc.as_deref(), Some("One SPI byte."));
    }

    /// Signs follow the declaration through aliases and into struct fields;
    /// widths and layout are the port's.
    #[test]
    fn signs_names_and_layout_come_from_the_declaration() {
        let probe = probe();
        let y = &probe.method("plain").unwrap().args[1];
        assert_eq!((y.ty.as_str(), y.width, y.signed), ("word", 16, true));
        assert!(probe.method("signed_ret").unwrap().result.as_ref().unwrap().signed);
        let p = &probe.method("pair").unwrap().args[0];
        let fields: Vec<(&str, u32, bool)> = p.fields.iter().map(|f| (f.name.as_str(), f.width, f.signed)).collect();
        assert_eq!(fields, [("a", 8, false), ("b", 4, true)]);
        assert_eq!(probe.doc.as_deref(), Some("A probe of every protocol."));
        assert_eq!(probe.method("plain").unwrap().doc.as_deref(), Some("Adds."));
    }

    #[test]
    fn the_description_round_trips_through_json() {
        let probe = probe();
        assert_eq!(Interface::from_json(&probe.to_json()).unwrap(), probe);
        let old = probe.to_json().replace("\"version\": 1", "\"version\": 0");
        assert!(Interface::from_json(&old).unwrap_err().contains("version 0"));
    }

    /// A disagreement between the declaration and the module is an error.
    #[test]
    fn a_port_or_a_function_unaccounted_for_is_refused() {
        let sources = |k: &str| vec![(Utf8PathBuf::from("probe.k"), k.to_owned())];
        let extra = PROBE_SV.replace("  output wire                                                stall_rate_supported_out", "  output wire stray_out,\n  output wire stall_rate_supported_out");
        let err = describe("Probe", &extra, &sources(PROBE_K)).unwrap_err();
        assert!(err.message().contains("no declared function explains: stray_out"), "{}", err.message());
        let renamed = PROBE_K.replace("uint16 handshake(uint8 x)", "uint16 handshake(uint8 z)");
        let err = describe("Probe", PROBE_SV, &sources(&renamed)).unwrap_err();
        assert!(err.message().contains("no port handshake_z_in"), "{}", err.message());
        let wrong = PROBE_K.replace("[[latency(1)]] uint9 lat1", "uint9 lat1");
        let err = describe("Probe", PROBE_SV, &sources(&wrong)).unwrap_err();
        assert!(err.message().contains("lat1 has no lat1_rdy_out"), "{}", err.message());
        let err = describe("Probe", PROBE_SV, &sources("class Other {}")).unwrap_err();
        assert!(err.message().contains("not declared in the project's sources"), "{}", err.message());
    }

    #[test]
    fn the_module_is_the_one_with_a_types_package() {
        let files = ["/g/Monitor_types.sv", "/g/Monitor.sv"].map(Utf8PathBuf::from);
        assert_eq!(module_name(&files).as_deref(), Some("Monitor"));
    }
}
