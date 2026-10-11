//! Owner-topic: **definitions** — what a name can resolve *to*.
//!
//! Spec: `docs/spec/resolve.md` §3.1 (the complete list of node kinds that
//! produce a symbol), §10 (how each declaration is keyed), §11.4 (`DefId`).
//! Reference: `Symbols.hs:37-71` (`getSymbols''`), `Parser/Syntax.hs:184-191`
//! (`DeclareFlag`).

use kg_base::{FileRange, SymbolPath};

use crate::module::ModuleSource;

/// Index of an item within one file's [`crate::lower::ItemTree`].
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct ItemId(pub u32);

impl ItemId {
    #[inline]
    pub fn index(self) -> usize {
        self.0 as usize
    }
}

/// A fully-qualified reference to a definition anywhere in the program
/// (`docs/spec/resolve.md` §11.4).
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct DefId {
    pub module: ModuleSource,
    pub item: ItemId,
}

impl DefId {
    pub fn new(module: ModuleSource, item: ItemId) -> DefId {
        DefId { module, item }
    }
}

/// `data DeclareFlag` (`Parser/Syntax.hs:184-191`) — how a variable was declared.
///
/// Load-bearing for the `shadow` warning, which never fires for `DeclareMember`
/// or `DeclareClass` (`TypeCheck.hs:112`), and for
/// `"Global state is not allowed"`, which fires for a non-`const`
/// `DeclareGlobal` (`TypeCheck.hs:126`).
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum DeclareFlag {
    /// File / module scope.
    DeclareGlobal,
    /// A class field.
    DeclareClass,
    /// A function-body local.
    DeclareLocal,
    DeclareStatic,
    /// A struct or union field.
    DeclareMember,
    DeclareUninitConst,
}

/// What kind of declaration a [`DefId`] points at.
///
/// This is exactly the set of node kinds `getSymbols''` inserts into the symbol
/// map (`Symbols.hs:52-68`) — `Alias`, `Capture`, `Struct`, `Union`, `Template`,
/// `Function`, `FunctionDecl`, `Enum`, `EnumConstant`, `Variable`, `FuncParam`,
/// `Class`, `Error` — plus [`DefKind::ExportType`], which is keyed under the
/// synthetic name `$exportN` (`SymbolTable.hs:213`), and
/// [`DefKind::TemplateParam`], which is a scope-stack entry rather than a global
/// symbol.
///
/// Nothing else produces a symbol. In particular **struct and union bodies are
/// not descended into** (`Symbols.hs:48-49`), so their fields never appear —
/// field access resolves through the *type* (`tField`, `Type.hs:1236`). Class
/// bodies *are* descended into.
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum DefKind {
    /// `using T = …;`
    Alias,
    /// A lambda capture (`Symbols.hs:53`).
    Capture,
    Struct,
    Union,
    /// A `class`. **Inserts two keys**: `scope ++ [name]` and
    /// `scope ++ [name, "$this"]` (`Symbols.hs:66`, `thisName = "$this"` at
    /// `Parser/Syntax.hs:267-268`).
    Class,
    Enum,
    EnumConstant,
    /// A function with a body.
    Function,
    /// A declaration without a body (`extern`, or a forward declaration).
    FunctionDecl,
    Variable(DeclareFlag),
    FuncParam,
    /// `template <...> decl`. Invisible under `SymbolsConfig::NoTemplate`, and
    /// `Template _ Typename` never produces a symbol at all (`Symbols.hs:58`).
    Template,
    /// A `template <typename T>` / `template <uint N>` parameter.
    TemplateParam,
    /// `export T;` — keyed `$export<n>` (`SymbolTable.hs:213`).
    ExportType,
    /// A parse or resolution error node. **All** error nodes collapse to the
    /// single key `["$errors"]` (`Symbols.hs:67`, `Parser/Syntax.hs:778`), which
    /// is the pipeline's "did anything fail" flag (`hasErrorSymbol`,
    /// `Symbols.hs:31-35`).
    Error,
}

impl DefKind {
    /// Whether this declaration names a *type*, which the parser needs in order
    /// to disambiguate `<` (`docs/spec/resolve.md` §7.1).
    pub fn is_type(self) -> bool {
        matches!(
            self,
            DefKind::Alias
                | DefKind::Struct
                | DefKind::Union
                | DefKind::Class
                | DefKind::Enum
                | DefKind::ExportType
        )
    }

    /// The two kinds `shadowable` accepts (`TypeCheck.hs:119-121`): a `Variable`
    /// with a non-empty scope, or a `FuncParam`. The scope test is the caller's.
    pub fn is_shadowable(self) -> bool {
        matches!(self, DefKind::Variable(_) | DefKind::FuncParam)
    }

    /// Whether `getSymbols''` files this declaration in the **post-parse** global
    /// map (`Symbols.hs:52-68`).
    ///
    /// Two kinds are deliberately absent from that `case`, so they fall through
    /// to `symbol _ = id` and never appear:
    ///
    /// * [`DefKind::TemplateParam`] — a `TemplateParam` node is seeded into the
    ///   parse-time scope stack by `unscopedWith [x]` (`Parser.hs:363`) and is
    ///   never passed to `parseSymbol`, so it is in neither map;
    /// * [`DefKind::ExportType`] — an `ExportType` node *is* passed to
    ///   `parseSymbol` (`Parser.hs:278`) and so lands in the module's flat
    ///   parse-time map under `$exportN` (`SymbolTable.hs:213`), which is what
    ///   `updateVisible`'s `exported ExportType{} = True` scans
    ///   (`SymbolTable.hs:175`) — but `getSymbols''` has no case for it.
    ///
    /// The two maps are genuinely different; see
    /// [`crate::lower::ItemTree::module_symbols`].
    pub fn is_global_symbol(self) -> bool {
        !matches!(self, DefKind::TemplateParam | DefKind::ExportType)
    }

    /// Whether the declaration went through `parseSymbol` and therefore landed in
    /// the module's flat parse-time map (`SymbolTable.hs:203-211`).
    ///
    /// The three kinds that did not: [`DefKind::Capture`] (built by bare `noted
    /// capture`, `Parser.hs:441,447`), [`DefKind::TemplateParam`]
    /// (`Parser.hs:362`) and [`DefKind::Error`]. The fourth exception — the
    /// declaration *wrapped* by a `template <...>`, which is `symbol`-ed as the
    /// `Template` node and never on its own (`Parser.hs:373-375`) — is a property
    /// of the item, not of its kind, so it lives on
    /// [`crate::lower::Item::is_declared`].
    pub fn is_parse_time_symbol(self) -> bool {
        !matches!(self, DefKind::Capture | DefKind::TemplateParam | DefKind::Error)
    }
}

/// `SymbolsConfig` (`Symbols.hs:28`) — the three ways `getSymbols'` can be built.
///
/// `docs/spec/resolve.md` §11.5: model these as three separate cached queries,
/// not as a parameter, so each is invalidated independently.
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Default)]
pub enum SymbolsConfig {
    /// Everything.
    #[default]
    AllSymbols,
    /// A `Template` node contributes `tail` — it *removes* the first entry its
    /// child produced and adds none, so templates become invisible
    /// (`Symbols.hs:59-60`).
    NoTemplate,
    /// A `Template` node contributes only its own key and its body is **not**
    /// traversed. Used by the inference fixpoint (`Inference.hs:37`).
    TemplateDecl,
}

/// One entry of the global symbol map.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct DefData {
    pub kind: DefKind,
    /// `getScope e ++ [getName e]` (`Symbols.hs:70-71`) — the key this
    /// declaration is filed under.
    pub qualified_name: SymbolPath,
    /// Where the declaration's name token is, for `symbolLoc` in redefinition and
    /// ambiguity messages (`SymbolTable.hs:199`).
    pub range: FileRange,
    /// Whether the declaration is the **direct** target of a `template <...>` —
    /// the `a` of `Template params a` (`Parser.hs:373-375`). Not "somewhere under
    /// a template": a member of a template class has `is_template == false`.
    /// See [`crate::lower::Item::is_template`], of which this is the
    /// cross-file image.
    pub is_template: bool,
}

impl DefData {
    /// `getScope e` (`Parser/Syntax.hs:780`) — `declScopeName`, i.e. the key with
    /// the declaration's own name dropped. Named to match
    /// [`crate::lower::Item::scope_name`].
    pub fn scope_name(&self) -> SymbolPath {
        self.qualified_name.init()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_variables_and_params_can_be_shadowed() {
        // TypeCheck.hs:119-121.
        assert!(DefKind::Variable(DeclareFlag::DeclareLocal).is_shadowable());
        assert!(DefKind::FuncParam.is_shadowable());
        assert!(!DefKind::Function.is_shadowable());
        assert!(!DefKind::EnumConstant.is_shadowable());
        assert!(!DefKind::Class.is_shadowable());
    }

    /// The `symbol` case list of `Symbols.hs:52-68`, transcribed as a membership
    /// test. Every kind named there is a global symbol; the two that are *not*
    /// named are not.
    #[test]
    fn the_global_symbol_set_is_exactly_the_symbols_hs_case_list() {
        for k in [
            DefKind::Alias,
            DefKind::Capture,
            DefKind::Struct,
            DefKind::Union,
            DefKind::Class,
            DefKind::Enum,
            DefKind::EnumConstant,
            DefKind::Function,
            DefKind::FunctionDecl,
            DefKind::Variable(DeclareFlag::DeclareGlobal),
            DefKind::FuncParam,
            DefKind::Template,
            DefKind::Error,
        ] {
            assert!(k.is_global_symbol(), "{k:?} is in the Symbols.hs:52-68 case list");
        }
        // `symbol _ = id` (Symbols.hs:68) catches these two.
        assert!(!DefKind::TemplateParam.is_global_symbol());
        assert!(!DefKind::ExportType.is_global_symbol());
    }

    #[test]
    fn captures_template_params_and_errors_never_reach_parse_symbol() {
        // Parser.hs:441,447 (capture), :362 (template parameter): neither is
        // wrapped in `symbol`. ExportType *is* (Parser.hs:278).
        assert!(!DefKind::Capture.is_parse_time_symbol());
        assert!(!DefKind::TemplateParam.is_parse_time_symbol());
        assert!(!DefKind::Error.is_parse_time_symbol());
        assert!(DefKind::ExportType.is_parse_time_symbol());
        assert!(DefKind::FuncParam.is_parse_time_symbol());
    }

    /// `is_type` is the parser's `<`-disambiguation oracle (§7.1); the set is
    /// `isType` restricted to the *declaration* forms (`Parser/Syntax.hs:676-690`:
    /// `AliasF`, `StructF`, `ClassF`, `UnionF`, `EnumF`, `TypeIdentifierF`).
    #[test]
    fn only_type_declarations_are_types() {
        for k in [DefKind::Alias, DefKind::Struct, DefKind::Union, DefKind::Class, DefKind::Enum] {
            assert!(k.is_type(), "{k:?}");
        }
        assert!(!DefKind::Function.is_type());
        assert!(!DefKind::Variable(DeclareFlag::DeclareLocal).is_type());
        assert!(!DefKind::EnumConstant.is_type());
    }
}
