//! Module graph, import resolution, scopes and name binding.
//!
//! Charter: the search order, the synthetic `.cmdargs` / `.options` modules, module
//! export lists and re-exports, and the scope stack with its shadowing rules.
//!
//! Spec: `docs/spec/resolve.md` — §1 search order, §2 exports, §3 `getSymbols`,
//! §4 scope kinds, §5 the synthetic modules, §7 why two passes are needed.
//!
//! This crate is deliberately free of `kg-ty`: a name resolves to a *declaration*,
//! and what type that declaration has is a later question.
//!
//! # Module map (owners, one per file)
//!
//! | module | owns | reference |
//! |---|---|---|
//! | [`def`] | [`DefId`], [`DefKind`], [`DeclareFlag`], [`SymbolsConfig`] | `Symbols.hs:28,52-68`, `Parser/Syntax.hs:184-191` |
//! | [`module`] | [`ModuleSource`], [`Namespace`], the search order, [`ModuleInterface`], imports | `Parser/IO.hs:37-110`, `SymbolTable.hs:94-181` |
//! | [`scope`] | [`ScopeKind`], [`ScopeStack`], [`Resolution`], `lookup_in_scope`, the two warnings | `SymbolTable.hs:51-278`, `Parser.hs:119-153` |
//! | [`lower`] | [`ItemTree`], the CST walk, `global_symbols` | `Symbols.hs:37-71`, `Parser.hs:108-241` |
//!
//! # Decisions the spec left open
//!
//! * `docs/spec/resolve.md` §11.3 sketches `enum ScopeKind { Module, Unscoped,
//!   Named, Quasi, Local, Function }` under a comment reading "§4.2, five kinds",
//!   and §4.2 itself lists **four** primitive combinators. Six variants are kept
//!   (see [`scope::ScopeKind`]); each documents which combinator it is.
//! * [`module::ModuleName`] components are `SmolStr`, not `Symbol`: a module-name
//!   component may contain `-` (`Parser/Lexer.hs:230-233`,
//!   `library/mini-base.k:12`) and so is not a legal identifier.
//! * Imports are stored in **source** order even though the reference stores them
//!   reversed; the reverse iteration is explicit at the one place it matters
//!   (`find_in_imports`). Storing them reversed makes the `redundant-import`
//!   attribution silently order-dependent.

pub mod def;
pub mod lower;
pub mod module;
pub mod scope;

pub use def::{DeclareFlag, DefData, DefId, DefKind, ItemId, SymbolsConfig};

pub use module::{
    CMDARGS_MODULE, ExportItem, ImportDecl, ImportError, ImportId, ImportNeeded, ModuleGraph,
    ModuleHeader, ModuleInterface, ModuleName, ModuleSource, Namespace, OPTIONS_MODULE,
    build_synthetic_module, module_file_name, resolve_module_path, search_dirs, split_name_value,
};

pub use scope::{
    Resolution, ScopeData, ScopeId, ScopeKind, ScopeStack, find_symbol, lookup_in_scope,
    qualifier_at, redundant_import_warning, scope_at, shadow_warning,
};

pub use lower::{
    DeclarationOrder, Item, ItemTree, LoweringScopes, global_symbols, lower_file, lower_file_named, names,
};
