//! The walk's calls kept as data, so they can be made again elsewhere.
//!
//! `kg build` walks into a [`Recording`] first and replays it into the real
//! backend only once the walk has completed (`isolate`): a program the walk
//! declines never reaches the library at all, where walking into `Real`
//! directly left it half-built when the decline came (AUDIT §4.4). The
//! replay happens in a worker process, which is what the byte encoding
//! ([`Recording::encode`]) is for.
//!
//! Every node-returning call mints a fresh id — unlike [`Trace`], which
//! gives `ParseAppendList` and `ParseEnumValue` their argument's id because
//! that is what the reference's pointers were measured to do. Replay maps
//! each id to whatever that very call returned, so it needs no belief about
//! which functions hand back an argument. And the walk only ever passes a
//! node back, never compares or hashes one (`B::Node` is opaque to it), so
//! the ids it was given cannot steer it: the calls recorded are the calls it
//! would have made into `Real`.
//!
//! [`Trace`]: crate::trace::Trace

use std::sync::OnceLock;

use kg_backend_sys::options::OptionValues;
use kg_pttrace::{Arg, Location};
use rustc_hash::FxHashMap;

use crate::trace::Ret;
use crate::{Backend, FUNCTIONS};

/// One call: the function, by its index in [`FUNCTIONS`], and its
/// arguments as the trace lifts them.
#[derive(Clone, Debug, PartialEq)]
pub struct Call {
    pub function: u16,
    pub args: Vec<Arg>,
}

impl Call {
    pub fn name(&self) -> &'static str {
        FUNCTIONS.get(usize::from(self.function)).copied().unwrap_or("?")
    }
}

/// The backend that keeps every call instead of making it.
#[derive(Debug, Default, PartialEq)]
pub struct Recording {
    calls: Vec<Call>,
    /// What `InitCompiler` was handed; the one argument a trace line does
    /// not carry in full.
    options: Option<OptionValues>,
    next: u64,
}

/// What a replayed call returned.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Replayed<N> {
    Node(N),
    Bool(bool),
    Int(i32),
    Void,
}

/// How far a replay got — [`crate::program::Outcome`]'s last two fields,
/// read off the backend that was replayed into.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Replay {
    /// `InitCompiler`'s answer; `false` stops the replay, as it stops the
    /// walk.
    pub initialised: bool,
    pub codegen: Option<bool>,
}

fn index_of(name: &str) -> u16 {
    static INDEX: OnceLock<FxHashMap<&'static str, u16>> = OnceLock::new();
    let index = INDEX.get_or_init(|| {
        FUNCTIONS.iter().enumerate().map(|(i, &n)| (n, u16::try_from(i).expect("fewer than 65536 functions"))).collect()
    });
    index[name]
}

const MAGIC: &[u8] = b"kg-ptbridge recording 1\n";

impl Recording {
    pub fn new() -> Recording {
        Recording::default()
    }

    /// The generated `Backend` impl's one entry point, shaped as
    /// `Trace::call` is so both impls are one text.
    pub(crate) fn call(&mut self, name: &'static str, args: &[Arg], ret: Ret) -> u64 {
        self.calls.push(Call { function: index_of(name), args: args.to_vec() });
        match ret {
            Ret::Node => {
                self.next += 1;
                self.next
            }
            // `InitCompiler` and `Codegen` say yes here; the replay reads
            // the real answers.
            Ret::Bool => 1,
            Ret::Int | Ret::Void => 0,
        }
    }

    pub(crate) fn options(&mut self, v: &OptionValues) {
        self.options = Some(v.clone());
    }

    pub fn calls(&self) -> &[Call] {
        &self.calls
    }

    /// Make every call again on `b`, in order, each recorded node id mapped
    /// to what `b` returned for the call that minted it. `before` sees each
    /// call's index and name before it is made — the worker's way of
    /// knowing which call a crash came out of.
    ///
    /// `Err` is a recording that does not describe a walk — an argument of
    /// the wrong shape, a node used before it was made — never an answer of
    /// the backend's.
    pub fn replay<B: Backend>(&self, b: &mut B, mut before: impl FnMut(usize, &Call)) -> Result<Replay, String> {
        let null = b.null();
        // Index `id` holds the node minted as `n<id>`; `0` is NULL.
        let mut nodes: Vec<B::Node> = Vec::with_capacity(self.next as usize + 1);
        nodes.push(null);
        let mut out = Replay { initialised: false, codegen: None };
        for (i, call) in self.calls.iter().enumerate() {
            before(i, call);
            let mut node = |a: &Arg| match a {
                Arg::Node(None) => Ok(null),
                Arg::Node(Some(id)) => usize::try_from(*id)
                    .ok()
                    .and_then(|i| nodes.get(i).copied())
                    .ok_or_else(|| format!("n{id} is used before a call made it")),
                other => Err(format!("a node was expected, the recording has {other:?}")),
            };
            let result = crate::generated::replay_call(b, usize::from(call.function), &call.args, self.options.as_ref(), &mut node)
                .map_err(|e| format!("call {i}: {e}"))?;
            match (result, call.name()) {
                (Replayed::Node(n), _) => nodes.push(n),
                (Replayed::Bool(ok), "InitCompiler") => {
                    if !ok {
                        return Ok(out);
                    }
                    out.initialised = true;
                }
                (Replayed::Bool(ok), "Codegen") => out.codegen = Some(ok),
                _ => {}
            }
        }
        Ok(out)
    }

    /// The bytes the worker reads: a magic line, the options as JSON, the
    /// node count, then each call as its function index and its tagged
    /// arguments, little-endian.
    pub fn encode(&self) -> Vec<u8> {
        let mut w = Vec::with_capacity(64 + self.calls.len() * 24);
        w.extend_from_slice(MAGIC);
        let options = self.options.as_ref().map(|o| serde_json::to_vec(o).expect("OptionValues serializes")).unwrap_or_default();
        put_len(&mut w, options.len());
        w.extend_from_slice(&options);
        w.extend_from_slice(&self.next.to_le_bytes());
        put_len(&mut w, self.calls.len());
        for call in &self.calls {
            w.extend_from_slice(&call.function.to_le_bytes());
            put_len(&mut w, call.args.len());
            for arg in &call.args {
                put_arg(&mut w, arg);
            }
        }
        w
    }

    /// The inverse of [`Recording::encode`].
    pub fn decode(bytes: &[u8]) -> Result<Recording, String> {
        let mut r = Reader { bytes, at: 0 };
        if r.take(MAGIC.len())? != MAGIC {
            return Err("not a kg-ptbridge recording".to_owned());
        }
        let n = r.len()?;
        let options = if n == 0 {
            None
        } else {
            Some(serde_json::from_slice(r.take(n)?).map_err(|e| format!("options: {e}"))?)
        };
        let next = r.u64()?;
        let count = r.len()?;
        let mut calls = Vec::with_capacity(count.min(1 << 20));
        for _ in 0..count {
            let function = u16::from_le_bytes(r.array()?);
            if usize::from(function) >= FUNCTIONS.len() {
                return Err(format!("function index {function} out of range"));
            }
            let argc = r.len()?;
            let args = (0..argc).map(|_| r.arg()).collect::<Result<Vec<_>, _>>()?;
            calls.push(Call { function, args });
        }
        if r.at != bytes.len() {
            return Err(format!("{} bytes after the last call", bytes.len() - r.at));
        }
        Ok(Recording { calls, options, next })
    }
}

fn put_len(w: &mut Vec<u8>, n: usize) {
    w.extend_from_slice(&u32::try_from(n).expect("under 4 GiB").to_le_bytes());
}

fn put_bytes(w: &mut Vec<u8>, b: &[u8]) {
    put_len(w, b.len());
    w.extend_from_slice(b);
}

fn put_arg(w: &mut Vec<u8>, arg: &Arg) {
    match arg {
        Arg::Node(None) => w.push(0),
        Arg::Node(Some(id)) => {
            w.push(1);
            w.extend_from_slice(&id.to_le_bytes());
        }
        Arg::Str(None) => w.push(2),
        Arg::Str(Some(b)) => {
            w.push(3);
            put_bytes(w, b);
        }
        Arg::Scope(None) => w.push(4),
        Arg::Scope(Some(items)) => {
            w.push(5);
            put_len(w, items.len());
            for item in items {
                put_bytes(w, item);
            }
        }
        Arg::UInt(x) => {
            w.push(6);
            w.extend_from_slice(&x.to_le_bytes());
        }
        Arg::Int(x) => {
            w.push(7);
            w.extend_from_slice(&x.to_le_bytes());
        }
        Arg::F32(x) => {
            w.push(8);
            w.extend_from_slice(&x.to_bits().to_le_bytes());
        }
        Arg::Loc(None) => w.push(9),
        Arg::Loc(Some(l)) => {
            w.push(10);
            for x in [l.begin_line, l.begin_column, l.end_line, l.end_column, l.file] {
                w.extend_from_slice(&(x as u64).to_le_bytes());
            }
            w.push(u8::from(l.valid));
        }
        Arg::Options => w.push(11),
        Arg::Void => w.push(12),
    }
}

struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], String> {
        let end = self.at.checked_add(n).filter(|&e| e <= self.bytes.len()).ok_or("the recording is cut short")?;
        let s = &self.bytes[self.at..end];
        self.at = end;
        Ok(s)
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N], String> {
        Ok(self.take(N)?.try_into().expect("take returned N bytes"))
    }

    fn len(&mut self) -> Result<usize, String> {
        Ok(u32::from_le_bytes(self.array()?) as usize)
    }

    fn u64(&mut self) -> Result<u64, String> {
        Ok(u64::from_le_bytes(self.array()?))
    }

    fn bytes(&mut self) -> Result<Vec<u8>, String> {
        let n = self.len()?;
        Ok(self.take(n)?.to_vec())
    }

    fn arg(&mut self) -> Result<Arg, String> {
        let [tag] = self.array()?;
        Ok(match tag {
            0 => Arg::Node(None),
            1 => Arg::Node(Some(self.u64()?)),
            2 => Arg::Str(None),
            3 => Arg::Str(Some(self.bytes()?)),
            4 => Arg::Scope(None),
            5 => {
                let n = self.len()?;
                Arg::Scope(Some((0..n).map(|_| self.bytes()).collect::<Result<_, _>>()?))
            }
            6 => Arg::UInt(self.u64()?),
            7 => Arg::Int(i64::from_le_bytes(self.array()?)),
            8 => Arg::F32(f32::from_bits(u32::from_le_bytes(self.array()?))),
            9 => Arg::Loc(None),
            10 => {
                let mut f = [0usize; 5];
                for x in &mut f {
                    *x = usize::try_from(self.u64()?).map_err(|_| "a location field overflows usize")?;
                }
                let [valid] = self.array()?;
                Arg::Loc(Some(Location {
                    begin_line: f[0],
                    begin_column: f[1],
                    end_line: f[2],
                    end_column: f[3],
                    file: f[4],
                    valid: valid != 0,
                }))
            }
            11 => Arg::Options,
            12 => Arg::Void,
            t => return Err(format!("unknown argument tag {t}")),
        })
    }
}

// The inverses of the generated lifts, for `replay_call`. A string was lifted
// as the bytes C receives (`ca_bytes`: one per character, the low one, up to
// the first NUL); mapping each byte back to the character with that code
// point makes a `&str` that `ca_bytes` turns into exactly those bytes again,
// whatever they were.

pub(crate) fn str_arg(a: &Arg) -> Result<Option<String>, String> {
    match a {
        Arg::Str(s) => Ok(s.as_ref().map(|b| b.iter().map(|&c| char::from(c)).collect())),
        other => Err(format!("a string was expected, the recording has {other:?}")),
    }
}

pub(crate) fn scope_arg(a: &Arg) -> Result<Vec<String>, String> {
    match a {
        Arg::Scope(Some(items)) => Ok(items.iter().map(|b| b.iter().map(|&c| char::from(c)).collect()).collect()),
        other => Err(format!("a scope was expected, the recording has {other:?}")),
    }
}

pub(crate) fn uint_arg(a: &Arg) -> Result<u32, String> {
    match a {
        Arg::UInt(x) => u32::try_from(*x).map_err(|_| format!("{x} does not fit an unsigned int")),
        other => Err(format!("an unsigned int was expected, the recording has {other:?}")),
    }
}

pub(crate) fn int_arg(a: &Arg) -> Result<i32, String> {
    match a {
        Arg::Int(x) => i32::try_from(*x).map_err(|_| format!("{x} does not fit an int")),
        other => Err(format!("an int was expected, the recording has {other:?}")),
    }
}

pub(crate) fn f32_arg(a: &Arg) -> Result<f32, String> {
    match a {
        Arg::F32(x) => Ok(*x),
        other => Err(format!("a float was expected, the recording has {other:?}")),
    }
}

pub(crate) fn loc_ptr_arg(a: &Arg) -> Result<Option<Location>, String> {
    match a {
        Arg::Loc(l) => Ok(*l),
        other => Err(format!("a location was expected, the recording has {other:?}")),
    }
}

pub(crate) fn loc_arg(a: &Arg) -> Result<Location, String> {
    loc_ptr_arg(a)?.ok_or_else(|| "a location by value is recorded as NULL".to_owned())
}
