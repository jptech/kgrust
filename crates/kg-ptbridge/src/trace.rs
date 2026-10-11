//! The trace-writing backend: every call becomes a `kg-pttrace` line, and
//! a node is a logical id minted in call order — the same first-sighting
//! numbering the recorder gives the reference's pointers, so two identical
//! call sequences carry identical ids.

use kg_backend_sys::options::OptionValues;
use kg_pttrace::{Arg, Writer};

/// What a call returns, for [`Trace::call`].
#[derive(Clone, Copy, Debug)]
pub enum Ret {
    Node,
    Bool,
    Int,
    Void,
}

/// Functions that return one of their arguments rather than a new node,
/// measured over the recorded corpus (`ParseAppendList` returns the list,
/// `ParseEnumValue` its value) — a new id here would put every later id
/// off by one.
const RETURNS_ARGUMENT: &[(&str, usize)] = &[("ParseAppendList", 0), ("ParseEnumValue", 1)];

pub struct Trace {
    writer: Writer,
    next: u64,
}

impl std::fmt::Debug for Trace {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Trace").field("next", &self.next).finish_non_exhaustive()
    }
}

/// `0` is `NULL`.
pub(crate) fn nz(id: u64) -> Option<u64> {
    (id != 0).then_some(id)
}

impl Trace {
    pub fn new(out: Box<dyn std::io::Write + Send>) -> Trace {
        let mut writer = Writer::new(out);
        let _ = writer.version("kg-ptbridge", "-");
        Trace { writer, next: 0 }
    }

    /// One line; the result is minted, or copied from the argument the real
    /// function returns.
    pub(crate) fn call(&mut self, name: &'static str, args: &[Arg], ret: Ret) -> u64 {
        let seq = self.writer.next_seq();
        let r = match ret {
            Ret::Node => match RETURNS_ARGUMENT.iter().find(|(n, _)| *n == name) {
                Some((_, i)) => match args[*i] {
                    Arg::Node(Some(id)) => id,
                    _ => 0,
                },
                None => {
                    self.next += 1;
                    self.next
                }
            },
            Ret::Bool => 1,
            Ret::Int | Ret::Void => 0,
        };
        let rendered = match ret {
            Ret::Node => Arg::Node(nz(r)),
            Ret::Bool => Arg::UInt(r),
            Ret::Int => Arg::Int(r as i64),
            Ret::Void => Arg::Void,
        };
        let _ = self.writer.call(seq, 0, name, args, &rendered);
        if name == "Codegen" {
            let _ = self.writer.epoch();
        }
        r
    }

    /// The `#options` line, rendered as the recorder renders the struct.
    pub(crate) fn options(&mut self, v: &OptionValues) {
        let m = kg_backend_sys::options::MarshalledOptions::new(v);
        let o = m.options();
        let mut s = String::new();
        s.push_str("device=");
        kg_pttrace::render_string(v.target_device.as_bytes(), &mut s);
        s.push_str(" cmd=");
        kg_pttrace::render_string(v.cmd_args.as_bytes(), &mut s);
        s.push_str(" files=");
        Arg::Scope(Some(v.file_names.iter().map(|f| f.as_bytes().to_vec()).collect())).render(&mut s);
        s.push_str(&format!(
            " frequency={} codegen={:?} placement={:?}",
            o._frequency, o._codegenOptions, o._placementOptions
        ));
        let _ = self.writer.options(&s.replace('\n', " "));
    }

    pub fn flush(&mut self) {
        let _ = self.writer.flush();
    }
}
