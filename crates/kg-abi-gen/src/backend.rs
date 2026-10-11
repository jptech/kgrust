//! `kg-ptbridge`'s `backend.rs`: the `Backend` trait — one method per
//! `extern "C"` function of the ABI, typed for Rust callers — its three
//! implementations, `Trace` (writes `kg-pttrace` lines), `Recording` (keeps
//! the calls, for the backend worker) and `Real` (calls `kg-backend-sys`),
//! and `replay_call`, which makes one recorded call on any of them. From the
//! headers via bindgen and `syn`, as `kg-ptrace`'s forwarders are, so none of
//! them can disagree with another. Until sprint W8.4 this was
//! `kg-ptbridge/build.rs`; the text it emits is the build script's, byte for
//! byte, under a banner.

use std::fmt::Write as _;

use quote::ToTokens;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    Node,
    CStr,
    Scope,
    UInt,
    Int,
    F32,
    LocPtr,
    Loc,
    Options,
}

impl Kind {
    /// The trait's parameter type.
    fn param(self) -> &'static str {
        match self {
            Kind::Node => "Self::Node",
            Kind::CStr => "Option<&str>",
            Kind::Scope => "&[&str]",
            Kind::UInt => "u32",
            Kind::Int => "i32",
            Kind::F32 => "f32",
            Kind::LocPtr => "Option<&kg_pttrace::Location>",
            Kind::Loc => "kg_pttrace::Location",
            Kind::Options => "&kg_backend_sys::options::OptionValues",
        }
    }
}

fn classify(ty: &syn::Type, enums: &std::collections::HashMap<String, Kind>, fn_name: &str) -> Kind {
    let text = ty.to_token_stream().to_string().replace(' ', "");
    match text.as_str() {
        "ParseTreeNodePtr" => Kind::Node,
        "*const::std::os::raw::c_char" => Kind::CStr,
        "ParseNamespaceScopePtr" | "ParseQualifiedNamePtr" => Kind::Scope,
        "::std::os::raw::c_uint" | "BOOL" => Kind::UInt,
        "::std::os::raw::c_int" => Kind::Int,
        "f32" => Kind::F32,
        "*constLocation" => Kind::LocPtr,
        "Location" => Kind::Loc,
        "*constOptions" => Kind::Options,
        _ => match enums.get(text.strip_suffix("::Type").unwrap_or(&text)) {
            Some(k) => *k,
            None => panic!("unknown ABI type `{text}` on {fn_name}; extend the table in kg-abi-gen/src/backend.rs"),
        },
    }
}

/// The module's text from bindgen's output over the headers — plain, with
/// the C ABI the headers declare.
fn text(bindings: &str) -> String {
    let file: syn::File = syn::parse_str(bindings).expect("bindgen output parses");
    let mut enums = std::collections::HashMap::new();
    for item in &file.items {
        if let syn::Item::Mod(m) = item {
            for inner in m.content.iter().flat_map(|(_, items)| items) {
                if let syn::Item::Type(t) = inner
                    && t.ident == "Type"
                {
                    let base = t.ty.to_token_stream().to_string().replace(' ', "");
                    let kind = match base.as_str() {
                        "::std::os::raw::c_uint" => Kind::UInt,
                        "::std::os::raw::c_int" => Kind::Int,
                        other => panic!("enum {} over {other}", m.ident),
                    };
                    let ident = m.ident.to_string();
                    enums.insert(ident.trim_start_matches('_').to_owned(), kind);
                    enums.insert(ident, kind);
                }
            }
        }
    }

    let mut trait_out = String::new();
    let mut trace_out = String::new();
    let mut real_out = String::new();
    let mut replay_out = String::new();
    let mut names = Vec::new();
    for item in &file.items {
        let syn::Item::ForeignMod(fm) = item else { continue };
        if fm.abi.name.as_ref().map(|n| n.value()) != Some("C".to_owned()) {
            continue;
        }
        for fi in &fm.items {
            let syn::ForeignItem::Fn(f) = fi else { continue };
            let name = f.sig.ident.to_string();
            if crate::UNDEFINED_SYMBOLS.contains(&name.as_str()) {
                continue;
            }
            names.push(name.clone());
            let params: Vec<(String, Kind)> = f
                .sig
                .inputs
                .iter()
                .enumerate()
                .map(|(i, input)| {
                    let syn::FnArg::Typed(pt) = input else { panic!("{name}: self parameter") };
                    (format!("a{i}"), classify(&pt.ty, &enums, &name))
                })
                .collect();
            let ret = match &f.sig.output {
                syn::ReturnType::Default => None,
                syn::ReturnType::Type(_, ty) => Some(classify(ty, &enums, &name)),
            };
            let ret_ty = match ret {
                None => "",
                Some(Kind::Node) => " -> Self::Node",
                Some(Kind::UInt) => " -> bool",
                Some(Kind::Int) => " -> i32",
                Some(k) => panic!("{name}: unexpected result kind {k:?}"),
            };
            let sig_params =
                params.iter().map(|(n, k)| format!("{n}: {}", k.param())).collect::<Vec<_>>().join(", ");
            let _ = writeln!(trait_out, "    #[allow(non_snake_case)]\n    fn {name}(&mut self{}{sig_params}){ret_ty};", if params.is_empty() { "" } else { ", " });

            // Trace: lift every argument, ask `call` for the result.
            let args = params
                .iter()
                .map(|(n, k)| match k {
                    Kind::Node => format!("Arg::Node(nz({n}))"),
                    Kind::CStr => format!("Arg::Str({n}.map(kg_backend_sys::ca_bytes))"),
                    Kind::Scope => format!("Arg::Scope(Some({n}.iter().map(|s| kg_backend_sys::ca_bytes(s)).collect()))"),
                    Kind::UInt => format!("Arg::UInt(u64::from({n}))"),
                    Kind::Int => format!("Arg::Int(i64::from({n}))"),
                    Kind::F32 => format!("Arg::F32({n})"),
                    Kind::LocPtr => format!("Arg::Loc({n}.copied())"),
                    Kind::Loc => format!("Arg::Loc(Some({n}))"),
                    Kind::Options => format!("{{ self.options({n}); Arg::Options }}"),
                })
                .collect::<Vec<_>>()
                .join(", ");
            let (ret_kind, ret_expr) = match ret {
                None => ("Ret::Void", ";"),
                Some(Kind::Node) => ("Ret::Node", ""),
                Some(Kind::UInt) => ("Ret::Bool", " != 0"),
                Some(Kind::Int) => ("Ret::Int", " as i32"),
                Some(_) => unreachable!(),
            };
            let sig_params_self = params
                .iter()
                .map(|(n, k)| format!("{n}: {}", k.param().replace("Self::Node", "u64")))
                .collect::<Vec<_>>()
                .join(", ");
            let _ = write!(
                trace_out,
                "    fn {name}(&mut self{}{sig_params_self}){} {{\n        let args = [{args}];\n        self.call(\"{name}\", &args, {ret_kind}){ret_expr}\n    }}\n",
                if params.is_empty() { "" } else { ", " },
                ret_ty.replace("Self::Node", "u64")
            );

            // Replay: each argument back out of its `Arg`, the inverse of the
            // lift above; the method is the trait's, so the result is
            // whatever the target backend returns.
            let index = names.len() - 1;
            let mut unpack = String::new();
            let mut pass = Vec::new();
            for (i, (n, k)) in params.iter().enumerate() {
                let getter = match k {
                    Kind::Node => "node",
                    Kind::CStr => "str_arg",
                    Kind::Scope => "scope_arg",
                    Kind::UInt => "uint_arg",
                    Kind::Int => "int_arg",
                    Kind::F32 => "f32_arg",
                    Kind::LocPtr => "loc_ptr_arg",
                    Kind::Loc => "loc_arg",
                    Kind::Options => "",
                };
                match k {
                    Kind::Options => {
                        let _ = writeln!(unpack, "            let {n} = options.ok_or(\"{name}: no options were recorded\")?;");
                    }
                    _ => {
                        let _ = writeln!(unpack, "            let {n} = {getter}(&args[{i}])?;");
                    }
                }
                pass.push(match k {
                    Kind::CStr => format!("{n}.as_deref()"),
                    Kind::Scope => format!("&{n}.iter().map(String::as_str).collect::<Vec<_>>()"),
                    Kind::LocPtr => format!("{n}.as_ref()"),
                    _ => n.clone(),
                });
            }
            let (wrap_open, wrap_close) = match ret {
                None => ("{ ", "; Replayed::Void }"),
                Some(Kind::Node) => ("Replayed::Node(", ")"),
                Some(Kind::UInt) => ("Replayed::Bool(", ")"),
                Some(Kind::Int) => ("Replayed::Int(", ")"),
                Some(_) => unreachable!(),
            };
            let _ = write!(
                replay_out,
                "        {index} => {{\n            if args.len() != {len} {{\n                return Err(format!(\"{name}: {{}} arguments recorded, {len} expected\", args.len()));\n            }}\n{unpack}            Ok({wrap_open}b.{name}({}){wrap_close})\n        }}\n",
                pass.join(", "),
                len = params.len(),
            );

            // Real: marshal, call, unmarshal.
            let mut prelude = String::new();
            let mut call_args = Vec::new();
            for (n, k) in &params {
                let i = &n[1..];
                match k {
                    Kind::Node | Kind::UInt | Kind::Int | Kind::F32 => call_args.push(n.clone()),
                    Kind::CStr => {
                        let _ = writeln!(prelude, "        let c{i} = {n}.map(kg_backend_sys::ca_string);");
                        call_args.push(format!("c{i}.as_ref().map_or(std::ptr::null(), |c| c.as_ptr())"));
                    }
                    Kind::Scope => {
                        let _ = writeln!(prelude, "        let s{i} = kg_backend_sys::options::StringArray::new({n}.iter().copied());");
                        call_args.push(format!("s{i}.as_ptr()"));
                    }
                    Kind::LocPtr => {
                        let _ = writeln!(prelude, "        let l{i} = {n}.map(crate::real::raw_location);");
                        call_args.push(format!("l{i}.as_ref().map_or(std::ptr::null(), std::ptr::from_ref)"));
                    }
                    Kind::Loc => call_args.push(format!("crate::real::raw_location(&{n})")),
                    Kind::Options => {
                        let _ = writeln!(prelude, "        let o{i} = kg_backend_sys::options::MarshalledOptions::new({n});");
                        call_args.push(format!("o{i}.as_ptr()"));
                    }
                }
            }
            let sig_params_real = params
                .iter()
                .map(|(n, k)| format!("{n}: {}", k.param().replace("Self::Node", "kg_backend_sys::ParseTreeNodePtr")))
                .collect::<Vec<_>>()
                .join(", ");
            let (open, post) = match ret {
                None | Some(Kind::Node) | Some(Kind::Int) => ("", ""),
                Some(Kind::UInt) => ("(", ") != 0"),
                Some(_) => unreachable!(),
            };
            let _ = write!(
                real_out,
                "    fn {name}(&mut self{}{sig_params_real}){} {{\n{prelude}        // SAFETY: arguments marshalled above, alive across the call; the protocol is the caller's.\n        {open}unsafe {{ self.api.{name}({}) }}{post}\n    }}\n",
                if params.is_empty() { "" } else { ", " },
                ret_ty.replace("Self::Node", "kg_backend_sys::ParseTreeNodePtr"),
                call_args.join(", ")
            );
        }
    }

    let out = format!(
        "/// The ParseTree ABI as a trait: one method per exported function, in header order.\npub trait Backend {{\n    type Node: Copy + PartialEq + std::fmt::Debug;\n    fn null(&self) -> Self::Node;\n{trait_out}}}\n\n#[allow(non_snake_case)]\nimpl Backend for crate::trace::Trace {{\n    type Node = u64;\n    fn null(&self) -> u64 {{ 0 }}\n{trace_out}}}\n\n// The same text as `Trace`'s: `Recording` has a `call` and an `options` of\n// the same shape, so the two lift every argument identically.\n#[allow(non_snake_case)]\nimpl Backend for crate::record::Recording {{\n    type Node = u64;\n    fn null(&self) -> u64 {{ 0 }}\n{trace_out}}}\n\n/// Make the recorded call `index` (into [`FUNCTIONS`]) on `b`, each node\n/// argument mapped through `node`.\n#[allow(non_snake_case, clippy::len_zero)]\npub(crate) fn replay_call<B: Backend>(\n    b: &mut B,\n    index: usize,\n    args: &[Arg],\n    options: Option<&kg_backend_sys::options::OptionValues>,\n    node: &mut dyn FnMut(&Arg) -> Result<B::Node, String>,\n) -> Result<Replayed<B::Node>, String> {{\n    match index {{\n{replay_out}        _ => Err(format!(\"no function {{index}}\")),\n    }}\n}}\n\n// A node is the backend's own opaque handle (`abi.md` §0.3), created and\n// owned by the library; passing one back is the protocol, not a dereference.\n#[allow(non_snake_case, clippy::not_unsafe_ptr_arg_deref)]\nimpl Backend for crate::real::Real {{\n    type Node = kg_backend_sys::ParseTreeNodePtr;\n    fn null(&self) -> Self::Node {{ std::ptr::null_mut() }}\n{real_out}}}\n\npub const FUNCTIONS: &[&str] = &[{}];\n",
        names.iter().map(|n| format!("\"{n}\"")).collect::<Vec<_>>().join(", ")
    );
    out
}

/// The checked-in `crates/kg-ptbridge/src/backend.rs` for the headers under
/// `cpp`, recorded as kanagawa `commit`'s.
pub fn backend(cpp: &std::path::Path, commit: &str) -> Result<String, String> {
    let digest = crate::headers_digest(cpp).map_err(|e| format!("reading the headers under {}: {e}", cpp.display()))?;
    let bindings = crate::configure(cpp)
        .formatter(bindgen::Formatter::None)
        .generate()
        .map_err(|e| format!("bindgen over {}: {e}", cpp.join(crate::HEADERS[0]).display()))?
        .to_string();
    let mut out = crate::banner("`kg-ptbridge`'s `Backend` trait, its three implementations, `replay_call`", commit, &digest);
    out.push_str(&text(&bindings));
    Ok(out)
}
