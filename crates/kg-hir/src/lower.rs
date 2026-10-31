//! CST → IR lowering: one file's typed syntax tree into a [`ModuleIr`].
//!
//! This walk reproduces the *tree-shaping* half of `Parser.hs` — every
//! desugaring the reference parser performs while building `ExpF` — without the
//! symbol table the reference consults as it goes:
//!
//! * `a ? b : c` becomes `Mux a (Seq [c, b])`, arms reversed (`Parser.hs:968`);
//! * `x op= e` becomes `Assign x (Binary op x' e)` with the lvalue re-read as an
//!   rvalue (`Parser.hs:631-638`); `x++`/`x--` add a literal `1`
//!   (`Parser.hs:607-614`);
//! * `atomic s` becomes `Annotated (Seq [FlagAttr Atomic]) s` (`Parser.hs:559-561`);
//! * a `{}` statement becomes a `NestedScope` named `$quasiN` (`Parser.hs:467-469`);
//! * the mandatory `break;` of a switch case is consumed (`Parser.hs:602-603`);
//! * a lone `;` in statement position becomes `Seq []` (`Parser.hs:482`) —
//!   but only as the **first** statement of its list: after any statement,
//!   `hidden (skipMany semi)` eats the rest without a node (`Parser.hs:479`).
//!
//! What it deliberately does **not** reproduce is the sorting of names:
//! every `PATH` lowers to [`Exp::Path`] and stays unsorted (divergence #22,
//! and the module comment of [`crate::exp`]).
//!
//! # Scope names
//!
//! `declScopeName` is the parser's scope qualifier at the moment each node is
//! built (`getScopeQualifier`, `SymbolTable.hs:96-97`). The qualifier algebra —
//! `$`, `$quasiN`, `?lambdaN`, and the `localScoped` pop that rewrites the
//! *parent's* qualifier — is [`kg_resolve::ScopeStack`], the same tested code
//! `kg_resolve::lower_file` walks with; the push/pop points below follow its
//! construct table (`kg-resolve/src/lower.rs:289-303`) exactly. One walk owns
//! the algebra; two walks share it; nothing re-implements it.
//!
//! # Provenance
//!
//! A node lowered *from* a CST node is `Own` at exactly that node's range and
//! kind — [`Lower::alloc`] takes the node, so the invariant holds by
//! construction. A node the lowering synthesizes (a `Seq` wrapper, the `1` of
//! `x++`, a lambda's default `auto`) is [`Src::donated`] from the nearest
//! enclosing CST node, which is what the reference's own synthesis does
//! (`newNote`, `Type.hs:983-984`).

use kg_base::{FileId, FileRange, Interner, SmolStr, Src, Symbol, SymbolPath, Written};
use kg_const::{BinOp, ConstInt, ConstValue, UnOp};
use kg_resolve::{DeclareFlag, ItemId, ScopeKind, ScopeStack};
use kg_syntax::{AstNode, SyntaxKind, SyntaxNode, ast};
use kg_ty::{Attribute, Flag, FunctionKind, QualifiedName};

use crate::arena::{ExpArena, ExpId};
use crate::exp::{Exp, Hole, Modifier, PathSeg, Refused, Segment, UnsortedPath};
use crate::sort::{
    FnInit, Hit, ImportSymbols, SymData, SymShape, SymTab, TemplateParamSym, TypeName,
    UnsortedReason,
};
use crate::{Import, ModuleIr};

/// Lowers one parsed file with **no symbol context**: every written path stays
/// one unsorted [`Exp::Path`] (divergence #22 — from a single file even a
/// local hit could be ambiguous with an import, `SymbolTable.hs:257`, so no
/// sort is safe alone). The `interner` must be the one every later comparison
/// and query resolves against; symbols from two interners do not compare.
pub fn lower(file: &ast::SourceFile, file_id: FileId, interner: &Interner) -> ModuleIr {
    lower_impl(file, file_id, interner, None)
}

/// Lowers one parsed file **sorting paths** the way the reference's parser
/// does (`Parser.hs:652-801`): each written name is resolved against an
/// incremental symbol table — declarations lowered so far, in order, plus
/// whatever `imports` exposes — and becomes the `ExpF` shape the reference
/// builds for it. Pass [`crate::sort::NoImports`] for a self-contained file
/// (the oracle's `--no-base` world); `kg-db` supplies the loaded program.
pub fn lower_sorted<'a>(
    file: &ast::SourceFile,
    file_id: FileId,
    interner: &'a Interner,
    imports: &'a dyn ImportSymbols,
) -> ModuleIr {
    lower_impl(file, file_id, interner, Some(SymTab::new(imports)))
}

fn lower_impl<'a>(
    file: &ast::SourceFile,
    file_id: FileId,
    interner: &'a Interner,
    sort: Option<SymTab<'a>>,
) -> ModuleIr {
    let mut lo = Lower {
        interner,
        file: file_id,
        arena: ExpArena::new(),
        scopes: ScopeStack::new(),
        member_of_class: false,
        in_member_function: false,
        sort,
        in_expr_type: false,
        in_template: false,
        template_params: Vec::new(),
        pending_decl: None,
        fn_type_attrs: None,
    };

    // `moduleDecl` pushes the namespace scope around the whole body
    // (`Parser.hs:189`); the header node itself is built outside it.
    let header = file.module_decl();
    let namespace = header.as_ref().and_then(|h| {
        let path = h.module_path()?;
        let name = kg_resolve::lower::module_path(&path);
        (!name.is_empty()).then(|| interner.intern(&kg_resolve::Namespace::mangle(&name)))
    });

    let header_parts = header.as_ref().map(|h| {
        let name = match h.module_path() {
            Some(p) => {
                let components = kg_resolve::lower::module_path(&p);
                lo.alloc(p.syntax(), Exp::ModuleIdentifier(components))
            }
            None => lo.synth(h.syntax(), Exp::ModuleIdentifier(Vec::new())),
        };
        let exposed = h
            .module_export_list()
            .map(|list| lo.export_list(&list))
            .unwrap_or_default();
        (name, exposed)
    });

    let imports = file
        .import_decls()
        .map(|i| Import {
            name: i.module_path().map(|p| kg_resolve::lower::module_path(&p)).unwrap_or_default(),
            alias: i
                .alias()
                .and_then(|n| n.ident_token())
                .map(|t| interner.intern(t.text())),
            range: FileRange::new(file_id, i.syntax().text_range()),
        })
        .collect();

    if let Some(ns) = namespace {
        lo.scopes.push(interner, ScopeKind::Module, Some(ns));
    }

    let mut items = Vec::new();
    for item in file.items() {
        if let Some(id) = lo.item(item.syntax()) {
            items.push(id);
        }
    }

    if namespace.is_some() {
        lo.scopes.pop(interner);
    }

    let root = match header_parts {
        Some((name, exposed)) => {
            let header_node = header.expect("header_parts implies a header");
            let body = lo.synth(header_node.syntax(), Exp::Seq(items));
            let decl =
                lo.alloc(header_node.syntax(), Exp::ModuleDecl { name, exposed, body });
            lo.alloc(file.syntax(), Exp::Seq(vec![decl]))
        }
        None => lo.alloc(file.syntax(), Exp::Seq(items)),
    };

    let sort_stats = lo.sort.take().map(|t| t.stats).unwrap_or_default();
    let lowered_len = lo.arena.len() as u32;
    ModuleIr { file: file_id, arena: lo.arena, root, imports, sort: sort_stats, lowered_len }
}

struct Lower<'a> {
    interner: &'a Interner,
    file: FileId,
    arena: ExpArena,
    scopes: ScopeStack,
    /// Whether the current declaration list is a `class` body — the one context
    /// where a function is a `MemberFunction` and a variable `DeclareClass`
    /// (`Parser.hs:341-350,462`).
    member_of_class: bool,
    /// Whether a member function's body is being lowered, at any depth:
    /// `localMemberFunctions` (`Frontend.hs:219-225`, pass 1) retags every
    /// function nested in a member function `MemberFunction`, which is what
    /// `addLambdaWrappers` reads when such a function is named as an
    /// argument — its wrapper captures `this` and calls through it
    /// (`Function.hs:406-418`; `local-functions.k::34`, 2026-09-23).
    in_member_function: bool,
    /// The incremental symbol table paths sort against — `None` disables
    /// sorting entirely (the historical, single-file behaviour).
    sort: Option<SymTab<'a>>,
    /// Whether the type being lowered was written in *expression* position —
    /// `try (typ' False)` (`Parser.hs:849`), where `typeSpecifier'` does not
    /// allow unresolved names. Set by the `Expr::Type` arm, cleared at every
    /// boundary the reference re-enters `typ` (= `typ' True`): `cast`,
    /// declaration types, and so on — see [`Lower::ty_decl`].
    in_expr_type: bool,
    /// Whether the declaration being lowered is the direct target of a
    /// `template <…>` — `template def = symbol $ Template …`
    /// (`Parser.hs:373-375`) declares the *template* under the inner name, so
    /// the inner declaration must not declare itself.
    in_template: bool,
    /// The lowered `template <…>` parameters, handed from [`Lower::template_decl`]
    /// to the wrapped declaration, which seeds them into its own scope — the
    /// `params` each of `alias'`/`struct'`/`class'`/`functionDef` re-seeds
    /// (`Parser.hs:276,299,336,408`).
    template_params: Vec<ItemId>,
    /// The wrapped declaration's name and shape, reported back to
    /// [`Lower::template_decl`] instead of being declared (see `in_template`).
    pending_decl: Option<(Symbol, SymShape)>,
    /// A variable's leading `[[…]]` the reference parses as its function
    /// type's (see [`Lower::var_decl`]), for the `FnType` arm to take.
    fn_type_attrs: Option<Vec<ast::AttrList>>,
}

impl<'a> Lower<'a> {
    // --- allocation ---------------------------------------------------------

    fn own_src(&self, node: &SyntaxNode) -> Src {
        Src::own(FileRange::new(self.file, node.text_range()))
    }

    /// A node lowered from `node`: `Own`, at `node`'s range, wearing `node`'s
    /// kind. The only way `Own` is ever produced, so the provenance gate's
    /// "kind matches at exactly that range" holds by construction.
    fn alloc(&mut self, node: &SyntaxNode, exp: Exp) -> ExpId {
        self.arena.alloc(exp, self.own_src(node), node.kind())
    }

    /// A synthesized node wearing `donor`'s range, `Donated`.
    fn synth(&mut self, donor: &SyntaxNode, exp: Exp) -> ExpId {
        let src = Src::donated(self.own_src(donor));
        self.arena.alloc(exp, src, donor.kind())
    }

    /// A hole. Wears the node's range so the totality report can point at it,
    /// but `Donated`, never `Own`: a hole is not the lowering of that node,
    /// and must not claim the `(range, kind)` `TypeKey` the real node's
    /// lowering may also claim — the collision `xtask ir`'s provenance gate
    /// rejects.
    fn unsupported(&mut self, node: &SyntaxNode) -> ExpId {
        self.synth(node, Exp::Unsupported)
    }

    /// A parser error-recovery node. The reference's AST carries these too —
    /// every error registers an `ErrorF`, filed under the one `$errors` key
    /// (`Symbols.hs:67`) — so this is a lowering, not a hole; `Unsupported` is
    /// reserved for constructs the *lowering* does not know. The payload is
    /// the unparsed text until the diagnostics slice threads a real message.
    ///
    /// One `ERROR` shape is not an error at all: `default = e;` — the
    /// `SyntaxKind` vocabulary has no `DEFAULT_INIT`, so the parser wraps the
    /// member form (`Parser.hs:314`) in an `ERROR` node without registering a
    /// message (`kg-parser/src/grammar/tests.rs:1052-1053` documents the gap).
    /// It lowers to the real `DefaultInitializationF`.
    fn error_node(&mut self, node: &SyntaxNode) -> ExpId {
        let mut tokens = node
            .children_with_tokens()
            .filter_map(|e| e.into_token())
            .filter(|t| !t.kind().is_trivia())
            .map(|t| t.kind());
        let mut first = tokens.next();
        if first == Some(SyntaxKind::STATIC_KW) {
            // `try $ keyword "static" *> defaultInitialization`
            // (`Parser.hs:475`) discards the `static`.
            first = tokens.next();
        }
        if first == Some(SyntaxKind::DEFAULT_KW)
            && let Some(expr) = node.children().find_map(ast::Expr::cast)
        {
            let value = self.expr(&expr, false);
            return self.alloc(node, Exp::DefaultInitialization(value));
        }
        let text = SmolStr::new(node.text().to_string());
        self.alloc(node, Exp::Error(crate::ErrorNote::new(text)))
    }

    fn qualifier(&self) -> QualifiedName {
        self.scopes.qualifier().clone()
    }

    fn push(&mut self, kind: ScopeKind, name: Option<Symbol>) {
        self.scopes.push(self.interner, kind, name);
    }

    fn pop(&mut self) {
        self.scopes.pop(self.interner);
    }

    fn name_symbol(&self, name: Option<ast::Name>) -> Symbol {
        match name.and_then(|n| n.ident_token()) {
            Some(token) => self.interner.intern(token.text()),
            None => self.interner.intern(""),
        }
    }

    /// `identifier = noted $ Identifier <$> name` (`Parser.hs:268`).
    fn identifier(&mut self, name: Option<ast::Name>, donor: &SyntaxNode) -> ExpId {
        match name {
            Some(n) => {
                let sym = self.name_symbol(Some(n.clone()));
                self.alloc(n.syntax(), Exp::Identifier(sym))
            }
            None => {
                let sym = self.interner.intern("");
                self.synth(donor, Exp::Identifier(sym))
            }
        }
    }

    // --- module header ------------------------------------------------------

    /// The exposure list. Entries are flat in the CST — `module a.b` is a
    /// `MODULE_KW` token plus a `MODULE_PATH`, `module a.b \ c.d` two paths
    /// around a `BACKSLASH`, and a name a `NAME_REF` — see the walk in
    /// `kg-resolve/src/lower.rs:1178-1196` and the pinned shape it cites.
    fn export_list(&mut self, list: &ast::ModuleExportList) -> Vec<ExpId> {
        let mut out = Vec::new();
        let mut lhs: Option<Vec<SmolStr>> = None;
        let mut rhs: Option<Vec<SmolStr>> = None;
        let mut after_backslash = false;
        let list_node = list.syntax().clone();

        for element in list.syntax().children_with_tokens() {
            if let Some(token) = element.as_token() {
                match token.kind() {
                    SyntaxKind::COMMA | SyntaxKind::R_CURLY => {
                        self.flush_export(&list_node, &mut lhs, &mut rhs, &mut out);
                        after_backslash = false;
                    }
                    SyntaxKind::BACKSLASH => after_backslash = true,
                    _ => {}
                }
                continue;
            }
            let Some(node) = element.as_node() else { continue };
            match node.kind() {
                SyntaxKind::MODULE_PATH => {
                    let path = ast::ModulePath::cast(node.clone()).expect("kind checked");
                    let components = kg_resolve::lower::module_path(&path);
                    if after_backslash {
                        rhs = Some(components);
                    } else {
                        lhs = Some(components);
                    }
                }
                SyntaxKind::NAME_REF => {
                    self.flush_export(&list_node, &mut lhs, &mut rhs, &mut out);
                    let sym = node
                        .first_token()
                        .map(|t| self.interner.intern(t.text()))
                        .unwrap_or_else(|| self.interner.intern(""));
                    out.push(self.alloc(node, Exp::Identifier(sym)));
                }
                _ => {}
            }
        }
        self.flush_export(&list_node, &mut lhs, &mut rhs, &mut out);
        out
    }

    fn flush_export(
        &mut self,
        list_node: &SyntaxNode,
        lhs: &mut Option<Vec<SmolStr>>,
        rhs: &mut Option<Vec<SmolStr>>,
        out: &mut Vec<ExpId>,
    ) {
        if let Some(l) = lhs.take() {
            let exp = match rhs.take() {
                Some(r) => Exp::ModuleDiff(l, r),
                None => Exp::ModuleIdentifier(l),
            };
            out.push(self.synth(list_node, exp));
        }
    }

    // --- items --------------------------------------------------------------

    /// One declaration-position node. `None` for the stray `;`s that
    /// `hidden (skipMany semi)` swallows without a node (`Parser.hs:251,266`).
    fn item(&mut self, node: &SyntaxNode) -> Option<ExpId> {
        Some(match node.kind() {
            SyntaxKind::EMPTY_STMT => return None,
            SyntaxKind::ERROR => self.error_node(node),
            SyntaxKind::CLASS_DECL => self.class_decl(node),
            SyntaxKind::STRUCT_DECL | SyntaxKind::UNION_DECL => self.aggregate_decl(node),
            SyntaxKind::ENUM_DECL => self.enum_decl(node),
            SyntaxKind::FN_DECL | SyntaxKind::FN_PROTO => self.function(node),
            SyntaxKind::VAR_DECL => self.var_decl(node, self.default_var_flag()),
            SyntaxKind::ALIAS_DECL => self.alias_decl(node),
            SyntaxKind::TEMPLATE_DECL => self.template_decl(node),
            SyntaxKind::EXPORT_DECL => self.export_decl(node),
            SyntaxKind::EXTERN_DECL => self.extern_decl(node),
            SyntaxKind::STATIC_ASSERT_DECL => self.static_assert(node),
            SyntaxKind::VISIBILITY => self.visibility(node),
            SyntaxKind::STATIC_IF_STMT => self.static_if(node, BranchScope::Quasi),
            _ => self.unsupported(node),
        })
    }

    /// The `variable` flavour this declaration position implies
    /// (`Parser.hs:461-465`). Statement positions override with
    /// `DeclareLocal`/`DeclareStatic`; `for` heads with `DeclareUninitConst`.
    fn default_var_flag(&self) -> DeclareFlag {
        if self.member_of_class { DeclareFlag::DeclareClass } else { DeclareFlag::DeclareGlobal }
    }

    fn class_decl(&mut self, node: &SyntaxNode) -> ExpId {
        let decl = ast::ClassDecl::cast(node.clone()).expect("kind checked");
        let wrapped = std::mem::replace(&mut self.in_template, false);
        let scope = self.qualifier();
        let name_sym = self.name_symbol(decl.name());
        let name = self.identifier(decl.name(), node);

        // `unscopedWith symbols $ scopedWith params ident` (`Parser.hs:335-336`).
        self.push(ScopeKind::Unscoped, None);
        // The lookahead pre-parse's seeds: the class and its type/variable/
        // template members (`Parser.hs:315-321,325-329`).
        self.preseed_class(wrapped, name_sym, &scope, decl.decl_list().as_ref());
        self.push(ScopeKind::Named, Some(name_sym));
        // `scopedWith params ident` — the template parameters land in the
        // class's own scope (`Parser.hs:336`).
        for id in std::mem::take(&mut self.template_params) {
            self.reseed(id);
        }

        let was_member = std::mem::replace(&mut self.member_of_class, true);
        let mut members = Vec::new();
        if let Some(list) = decl.decl_list() {
            for child in list.syntax().children() {
                if let Some(id) = self.item(&child) {
                    members.push(id);
                }
            }
        }
        self.member_of_class = was_member;

        self.pop();
        self.pop();

        let body = match decl.decl_list() {
            Some(list) => self.alloc(list.syntax(), Exp::Seq(members)),
            None => self.synth(node, Exp::Seq(members)),
        };
        let id = self.alloc(node, Exp::Class { scope: scope.clone(), name, body });
        self.finish_decl(wrapped, name_sym, &scope, SymShape::Type, id);
        id
    }

    /// `struct'` / `union'` (`Parser.hs:298-305`): a `Quasi` scope, members are
    /// `uninitVar DeclareMember` — no attributes, no initializer in the
    /// reference's grammar; whatever *is* written is lowered rather than
    /// silently dropped.
    fn aggregate_decl(&mut self, node: &SyntaxNode) -> ExpId {
        let wrapped = std::mem::replace(&mut self.in_template, false);
        let scope = self.qualifier();
        let (name_ast, list) = match node.kind() {
            SyntaxKind::STRUCT_DECL => {
                let d = ast::StructDecl::cast(node.clone()).expect("kind checked");
                (d.name(), d.decl_list())
            }
            _ => {
                let d = ast::UnionDecl::cast(node.clone()).expect("kind checked");
                (d.name(), d.decl_list())
            }
        };
        let name_sym = self.name_symbol(name_ast.clone());
        let name = self.identifier(name_ast, node);

        self.push(ScopeKind::Quasi, None);
        // `quasiScopedWith params` (`Parser.hs:299,305`).
        for id in std::mem::take(&mut self.template_params) {
            self.reseed(id);
        }
        let was_member = std::mem::replace(&mut self.member_of_class, false);
        let mut members = Vec::new();
        if let Some(list) = &list {
            for child in list.syntax().children() {
                if child.kind() == SyntaxKind::VAR_DECL {
                    members.push(self.var_decl(&child, DeclareFlag::DeclareMember));
                } else if let Some(id) = self.item(&child) {
                    members.push(id);
                }
            }
        }
        self.member_of_class = was_member;
        self.pop();

        let body = match &list {
            Some(l) => self.alloc(l.syntax(), Exp::Seq(members)),
            None => self.synth(node, Exp::Seq(members)),
        };
        let id = match node.kind() {
            SyntaxKind::STRUCT_DECL => {
                self.alloc(node, Exp::Struct { scope: scope.clone(), name, body })
            }
            _ => self.alloc(node, Exp::Union { scope: scope.clone(), name, body }),
        };
        self.finish_decl(wrapped, name_sym, &scope, SymShape::Type, id);
        id
    }

    /// `enum` (`Parser.hs:282-292`): the constants live in a `Named` scope, and
    /// each constant after the first is one `Unscoped` deeper — the reference
    /// nests the continuation so an earlier constant is in scope for a later
    /// one's value.
    fn enum_decl(&mut self, node: &SyntaxNode) -> ExpId {
        let decl = ast::EnumDecl::cast(node.clone()).expect("kind checked");
        let wrapped = std::mem::replace(&mut self.in_template, false);
        // `Enum <$> getScopeQualifier` runs before `scoped ident`
        // (`Parser.hs:284`).
        let scope = self.qualifier();
        let name_sym = self.name_symbol(decl.name());
        let name = self.identifier(decl.name(), node);
        // The underlying type is parsed before the scope opens.
        let ty = match enum_base_type(&decl) {
            Some(t) => self.ty_decl(&t),
            None => self.synth(node, Exp::Unsupported),
        };

        self.push(ScopeKind::Named, Some(name_sym));
        let const_scope = self.qualifier();
        let mut consts = Vec::new();
        let variants: Vec<ast::EnumVariant> = enum_variants(&decl).collect();
        let extra_scopes = variants.len().saturating_sub(1);
        for (i, variant) in variants.iter().enumerate() {
            if i > 0 {
                self.push(ScopeKind::Unscoped, None);
            }
            let vname = self.identifier(variant.name(), variant.syntax());
            let value = variant.expr().map(|e| self.expr(&e, false));
            let cid = self.alloc(
                variant.syntax(),
                Exp::EnumConstant { scope: const_scope.clone(), name: vname, value },
            );
            consts.push(cid);
            // Each constant is a `symbol` (`Parser.hs:291`), visible to the
            // value of every later one.
            let vsym = self.name_symbol(variant.name());
            self.declare_now(vsym, &const_scope.clone(), SymShape::OtherValue, cid);
        }
        for _ in 0..extra_scopes {
            self.pop();
        }
        self.pop();

        let body = match decl.decl_list() {
            Some(l) => self.alloc(l.syntax(), Exp::Seq(consts)),
            None => self.synth(node, Exp::Seq(consts)),
        };
        let id = self.alloc(node, Exp::Enum { scope: scope.clone(), name, ty, body });
        self.finish_decl(wrapped, name_sym, &scope, SymShape::Type, id);
        id
    }

    /// `functionDef` (`Parser.hs:406-424`): scope captured first, then
    /// `Unscoped` (template params), `Unscoped` (self), `Named` (the name), and
    /// `Function` (`$`) around the body.
    fn function(&mut self, node: &SyntaxNode) -> ExpId {
        let wrapped = std::mem::replace(&mut self.in_template, false);
        let scope = self.qualifier();
        let kind = if self.member_of_class || self.in_member_function {
            FunctionKind::MemberFunction
        } else {
            FunctionKind::FreeFunction
        };

        self.push(ScopeKind::Unscoped, None);
        // `unscopedWith templateParams` (`Parser.hs:408`): the parameters are
        // in scope for the return type onward.
        for id in std::mem::take(&mut self.template_params) {
            self.reseed(id);
        }

        let (attr_lists, modifier_ast, ret_ast, name_ast, params_ast, body_ast) =
            match node.kind() {
                SyntaxKind::FN_DECL => {
                    let d = ast::FnDecl::cast(node.clone()).expect("kind checked");
                    (
                        d.attr_lists().collect::<Vec<_>>(),
                        d.fn_modifier(),
                        d.ret(),
                        d.name(),
                        d.param_list(),
                        d.body(),
                    )
                }
                _ => {
                    let d = ast::FnProto::cast(node.clone()).expect("kind checked");
                    (d.attr_lists().collect(), None, d.ret(), d.name(), d.param_list(), None)
                }
            };

        let attr = self.attrs(&attr_lists, node);
        let modifier = modifier_ast.map(|m| {
            let modi = if m.noinline_kw_token().is_some() {
                Modifier::NoInline
            } else {
                Modifier::Inline
            };
            self.alloc(m.syntax(), Exp::FunctionModifier(modi))
        });
        let ret = self.ty_decl_or_unsupported(ret_ast, node);
        let name_sym = self.name_symbol(name_ast.clone());
        let name = self.identifier(name_ast, node);

        self.push(ScopeKind::Unscoped, None);
        // `unscopedWith (maybeToList self)` (`Parser.hs:415`): only a function
        // *template* can see itself — `functionTemplate` pre-parses the
        // `Template` node and seeds it here (`Parser.hs:428-430`); a plain
        // function seeds nothing.
        if wrapped {
            self.seed_symbol(
                name_sym,
                scope.clone(),
                SymShape::Template { function: true, type_like: false },
            );
        }
        self.push(ScopeKind::Named, Some(name_sym));

        let params = self.param_list(params_ast, node);

        let out = match body_ast {
            Some(block) => {
                self.push(ScopeKind::Function, None);
                let was_member = std::mem::replace(&mut self.member_of_class, false);
                let was_in = std::mem::replace(&mut self.in_member_function, kind == FunctionKind::MemberFunction);
                let body = self.block_seq(&block);
                self.in_member_function = was_in;
                self.member_of_class = was_member;
                self.pop();
                self.alloc(
                    node,
                    Exp::Function {
                        scope: scope.clone(),
                        kind,
                        attr,
                        modifier,
                        ret,
                        name,
                        params,
                        body,
                    },
                )
            }
            None => self.alloc(
                node,
                Exp::FunctionDecl { scope: scope.clone(), attr, ret, name, params },
            ),
        };

        self.pop();
        self.pop();
        self.pop();
        self.finish_decl(wrapped, name_sym, &scope, SymShape::Function, out);
        out
    }

    /// `funcParams` (`Parser.hs:377-382`): each parameter's scope is the
    /// function's `Named` scope.
    fn param_list(&mut self, list: Option<ast::ParamList>, donor: &SyntaxNode) -> ExpId {
        let scope = self.qualifier();
        let mut out = Vec::new();
        if let Some(list) = &list {
            for param in list.params() {
                let attrs: Vec<ast::AttrList> = param.attr_lists().collect();
                let attr = self.attrs(&attrs, param.syntax());
                let ty = self.ty_decl_or_unsupported(param.ty(), param.syntax());
                let name = self.identifier(param.name(), param.syntax());
                let default = param.eq_token().and(param.expr()).map(|e| self.expr(&e, false));
                let pid = self.alloc(
                    param.syntax(),
                    Exp::FuncParam { scope: scope.clone(), attr, ty, name, default },
                );
                out.push(pid);
                // `symbol param` (`Parser.hs:378`) — a parameter is a value
                // symbol `resolveTypeName` ignores (it is not a `Variable`,
                // `Parser.hs:747`).
                let psym = self.name_symbol(param.name());
                self.declare_now(psym, &scope.clone(), SymShape::OtherValue, pid);
            }
        }
        match list {
            Some(l) => self.alloc(l.syntax(), Exp::Seq(out)),
            None => self.synth(donor, Exp::Seq(out)),
        }
    }

    fn alias_decl(&mut self, node: &SyntaxNode) -> ExpId {
        let decl = ast::AliasDecl::cast(node.clone()).expect("kind checked");
        let wrapped = std::mem::replace(&mut self.in_template, false);
        let scope = self.qualifier();
        let name_sym = self.name_symbol(decl.name());
        let name = self.identifier(decl.name(), node);
        // `unscopedWith params typ` (`Parser.hs:276`).
        self.push(ScopeKind::Unscoped, None);
        for id in std::mem::take(&mut self.template_params) {
            self.reseed(id);
        }
        let body = self.ty_decl_or_unsupported(decl.ty(), node);
        self.pop();
        // `Alias _ (TypeIdentifier _ t) _` is the shape later `::` prefixes
        // resolve through (`Parser.hs:667-668`).
        let target = match &self.arena[body].exp {
            Exp::TypeIdentifier(_, payload) => {
                Some(SymbolPath::new(self.spine_qualname(*payload)))
            }
            _ => None,
        };
        let id = self.alloc(node, Exp::Alias { scope: scope.clone(), body, name });
        self.finish_decl(wrapped, name_sym, &scope, SymShape::Alias { target }, id);
        id
    }

    /// `template def` (`Parser.hs:373-375`): the parameter list, then the
    /// wrapped declaration. Parameters after the first are parsed one
    /// `Unscoped` deeper (`Parser.hs:361-364`); the scope on every
    /// `TemplateParam` is the one captured *before* the list (`Parser.hs:353`).
    fn template_decl(&mut self, node: &SyntaxNode) -> ExpId {
        let decl = ast::TemplateDecl::cast(node.clone()).expect("kind checked");
        let scope = self.qualifier();
        let mut params = Vec::new();
        let mut param_syms: Vec<ItemId> = Vec::new();

        let mut pushed = 0usize;
        if let Some(list) = decl.template_param_list() {
            for (index, child) in list
                .syntax()
                .children()
                .filter(|c| {
                    matches!(c.kind(), SyntaxKind::TYPE_PARAM | SyntaxKind::VALUE_PARAM)
                })
                .enumerate()
            {
                if index > 0 {
                    self.push(ScopeKind::Unscoped, None);
                    pushed += 1;
                    // `unscopedWith [x] (comma >> paramList …)`
                    // (`Parser.hs:361-364`): the previous parameter is in
                    // scope for this one's kind and default.
                    if let Some(&prev) = param_syms.last() {
                        self.reseed(prev);
                    }
                }
                let (id, sym) = self.template_param(&child, &scope, index as u32);
                params.push(id);
                param_syms.extend(sym);
            }
        }
        for _ in 0..pushed {
            self.pop();
        }

        // `template def = symbol $ Template params <$> noted (def params)`
        // (`Parser.hs:373-375`): the wrapped declaration seeds the parameters
        // into its own scope and reports its name back instead of declaring.
        let saved_template = std::mem::replace(&mut self.in_template, true);
        let saved_params = std::mem::replace(&mut self.template_params, param_syms);
        self.pending_decl = None;
        let inner = match decl.decl() {
            Some(item) => self.item(item.syntax()).unwrap_or_else(|| {
                let n = item.syntax().clone();
                self.unsupported(&n)
            }),
            None => self.unsupported(node),
        };
        self.in_template = saved_template;
        self.template_params = saved_params;
        // Allocated before declaring: the reference's entry for a template is
        // the completed `Template` node itself (`symbol $ Template params …`,
        // `Parser.hs:373-375`), and that is the tree `nub` compares.
        let out = self.alloc(node, Exp::Template { params, decl: inner });
        if let Some((name, inner_shape)) = self.pending_decl.take() {
            let shape = SymShape::Template {
                function: inner_shape.is_function(),
                type_like: inner_shape.is_type(),
            };
            self.declare_now(name, &scope.clone(), shape, out);
        }
        out
    }

    /// One template parameter (`Parser.hs:366-371`): `typename T`,
    /// a non-type `uint32 N`, or the template-template
    /// `template <typename, auto> typename M`. Sorting also returns the
    /// parameter's table entry, seeded by the caller and by the wrapped
    /// declaration (`Parser.hs:361-364,408` etc.).
    fn template_param(
        &mut self,
        node: &SyntaxNode,
        scope: &QualifiedName,
        index: u32,
    ) -> (ExpId, Option<ItemId>) {
        match node.kind() {
            // The parser writes both `typename T = D` and the template-template
            // `template <typename, auto> typename M = D` **flat** under one
            // `TYPE_PARAM`: keyword tokens, bare kind nodes between the angles,
            // then `NAME` and the default. The generated `default_type()` is
            // `support::child` — the *first* `Type` child — which for the
            // template-template shape is an inner kind, not the default, so
            // this walks the children in order instead.
            SyntaxKind::TYPE_PARAM => {
                let p = ast::TypeParam::cast(node.clone()).expect("kind checked");
                let name = self.name_symbol(p.name());
                let mut inner_kinds = Vec::new();
                let mut has_template = false;
                let mut in_angles = false;
                let mut after_eq = false;
                let mut default = None;
                for element in node.children_with_tokens() {
                    if let Some(token) = element.as_token() {
                        match token.kind() {
                            SyntaxKind::TEMPLATE_KW => has_template = true,
                            SyntaxKind::L_ANGLE => in_angles = true,
                            SyntaxKind::R_ANGLE => in_angles = false,
                            SyntaxKind::EQ => after_eq = true,
                            _ => {}
                        }
                        continue;
                    }
                    let Some(child) = element.as_node() else { continue };
                    if child.kind() == SyntaxKind::NAME {
                        continue;
                    }
                    if after_eq {
                        if default.is_none()
                            && let Some(t) = ast::Type::cast(child.clone())
                        {
                            // `typenameParam`'s default is `typ`, but a
                            // template-template parameter's default is
                            // `templateSpecifier` (`Parser.hs:366,368`) —
                            // a bare template name wrapped `TypeIdentifier`.
                            default = Some(if has_template {
                                self.template_specifier_ty(&t)
                            } else {
                                self.ty_decl(&t)
                            });
                        }
                    } else if in_angles {
                        // `paramKind = typename <|> typ` (`Parser.hs:371`): a
                        // bare `typename` parses as a path-less TYPENAME_TYPE.
                        if child.kind() == SyntaxKind::TYPENAME_TYPE
                            && ast::TypenameType::cast(child.clone())
                                .and_then(|t| t.path_type())
                                .is_none()
                        {
                            inner_kinds.push(self.alloc(child, Exp::Typename));
                        } else if let Some(t) = ast::Type::cast(child.clone()) {
                            inner_kinds.push(self.ty_decl(&t));
                        }
                    }
                }
                let kind = if has_template {
                    // `typeTemplateSig = Template <$> angles (paramKind …) <*>
                    // typename` (`Parser.hs:370-371`).
                    let decl = self.synth(node, Exp::Typename);
                    self.synth(node, Exp::Template { params: inner_kinds, decl })
                } else {
                    self.synth(node, Exp::Typename)
                };
                let id = self.alloc(
                    node,
                    Exp::TemplateParam { scope: scope.clone(), kind, name, index, default },
                );
                let sym = self.template_param_sym(scope, name, index, kind);
                (id, sym)
            }
            SyntaxKind::VALUE_PARAM => {
                let p = ast::ValueParam::cast(node.clone()).expect("kind checked");
                let name = self.name_symbol(p.name());
                let kind = self.ty_decl_or_unsupported(p.ty(), node);
                let default = p.default_value().map(|e| self.expr(&e, false));
                let id = self.alloc(
                    node,
                    Exp::TemplateParam { scope: scope.clone(), kind, name, index, default },
                );
                let sym = self.template_param_sym(scope, name, index, kind);
                (id, sym)
            }
            _ => (self.unsupported(node), None),
        }
    }

    /// The table entry of one template parameter — the pieces of
    /// `TemplateParamF n a b c _` every use rebuilds as `TypeParamF n a b c`
    /// (`Parser.hs:658-666`).
    fn template_param_sym(
        &mut self,
        scope: &QualifiedName,
        name: Symbol,
        index: u32,
        kind: ExpId,
    ) -> Option<ItemId> {
        self.sort.as_ref()?;
        let param = TemplateParamSym {
            scope: scope.clone(),
            name,
            index,
            kind,
            kind_is_type: self.exp_is_type(kind),
            kind_is_template: self.exp_is_template(kind),
            kind_is_function_type: matches!(self.arena[kind].exp, Exp::FunctionType { .. }),
        };
        let tab = self.sort.as_mut().expect("checked");
        Some(tab.push(SymData {
            name,
            decl_scope: scope.clone(),
            shape: SymShape::TemplateParam(param),
            decl: None,
        }))
    }

    /// `isType` over lowered nodes (`Parser/Syntax.hs:676-699`).
    fn exp_is_type(&self, id: ExpId) -> bool {
        match &self.arena[id].exp {
            Exp::Auto
            | Exp::Void
            | Exp::Boolean
            | Exp::Float
            | Exp::Str
            | Exp::Const(_)
            | Exp::Reference(_)
            | Exp::Integer(_)
            | Exp::Unsigned(_)
            | Exp::ParamInt(_)
            | Exp::ParamUint(_)
            | Exp::Enum { .. }
            | Exp::Struct { .. }
            | Exp::Class { .. }
            | Exp::Union { .. }
            | Exp::Alias { .. }
            | Exp::TypeIdentifier(..)
            | Exp::Array { .. }
            | Exp::FunctionType { .. } => true,
            Exp::Template { decl, .. } => self.exp_is_type(*decl),
            Exp::TypeParam { kind, .. } | Exp::TemplateParam { kind, .. } => {
                !self.exp_is_type(*kind)
            }
            Exp::TemplateInstance { base, .. } => self.exp_is_type(*base),
            _ => false,
        }
    }

    /// `isTemplate` over lowered nodes (`Parser/Syntax.hs:701-704`).
    fn exp_is_template(&self, id: ExpId) -> bool {
        match &self.arena[id].exp {
            Exp::Template { .. } => true,
            Exp::TypeParam { kind, .. } | Exp::TemplateParam { kind, .. } => {
                self.exp_is_template(*kind)
            }
            _ => false,
        }
    }

    fn export_decl(&mut self, node: &SyntaxNode) -> ExpId {
        let decl = ast::ExportDecl::cast(node.clone()).expect("kind checked");
        let attrs: Vec<ast::AttrList> = decl.attr_lists().collect();
        let attr = self.attrs(&attrs, node);
        let ty = self.export_ty(decl.ty(), node);
        self.alloc(node, Exp::ExportType { attr, ty })
    }

    fn extern_decl(&mut self, node: &SyntaxNode) -> ExpId {
        let decl = ast::ExternDecl::cast(node.clone()).expect("kind checked");
        let attrs: Vec<ast::AttrList> = decl.attr_lists().collect();
        let attr = self.attrs(&attrs, node);
        let ty = self.export_ty(decl.ty(), node);
        self.alloc(node, Exp::Extern { attr, ty })
    }

    /// `try templateSpecifier <|> typ` (`Parser.hs:278,280`): a bare template
    /// name exports as `TypeIdentifier`; everything else is an ordinary type.
    fn export_ty(&mut self, t: Option<ast::Type>, donor: &SyntaxNode) -> ExpId {
        match t {
            Some(ast::Type::PathType(p)) if self.sorting() => match p.path() {
                Some(path) => self.path_pos(&path, PathPos::ExportTy, false),
                None => self.unsupported(p.syntax()),
            },
            other => self.ty_decl_or_unsupported(other, donor),
        }
    }

    /// `type''` in expression position (`Parser.hs:533-540`): nested index
    /// expressions whose innermost base is a path that sorts as a type are
    /// the reference's `Array (Seq []) elem (Seq dims)`. `None` means the
    /// written form really is an array access — the legitimate backtrack,
    /// not a decline, so nothing is counted.
    fn index_as_array_type(&mut self, outer: &ast::IndexExpr) -> Option<ExpId> {
        let mut dims_rev: Vec<ast::Expr> = Vec::new();
        let mut cur = outer.clone();
        let base_path = loop {
            dims_rev.push(cur.index()?);
            match cur.base()? {
                ast::Expr::IndexExpr(inner) => cur = inner,
                ast::Expr::Path(p) => break p,
                _ => return None,
            }
        };
        let segs: Vec<SegView> =
            base_path.path_segments().map(|s| seg_view(self.interner, &s)).collect();
        let elem = self
            .sort_segments(
                base_path.syntax(),
                &segs,
                PathPos::Ty { allow_unresolved: false },
            )
            .ok()?;
        let dims: Vec<ExpId> =
            dims_rev.iter().rev().map(|d| self.expr(d, false)).collect();
        let attr = self.synth(outer.syntax(), Exp::Seq(Vec::new()));
        let dims = self.synth(outer.syntax(), Exp::Seq(dims));
        Some(self.alloc(outer.syntax(), Exp::Array { attr, elem, dims }))
    }

    /// `templateSpecifier` for a written *type* — a template-template
    /// parameter's default (`Parser.hs:368,702-708`).
    fn template_specifier_ty(&mut self, t: &ast::Type) -> ExpId {
        match t {
            ast::Type::PathType(p) if self.sorting() => match p.path() {
                Some(path) => self.path_pos(&path, PathPos::TemplateArg, false),
                None => self.unsupported(p.syntax()),
            },
            other => self.ty_decl(other),
        }
    }

    /// Both spellings (`Parser.hs:571-574`); the token decides which node.
    fn static_assert(&mut self, node: &SyntaxNode) -> ExpId {
        let decl = ast::StaticAssertDecl::cast(node.clone()).expect("kind checked");
        let expr = match decl.expr() {
            Some(e) => self.expr(&e, false),
            None => self.unsupported(node),
        };
        if decl.static_assert_kw_token().is_some() {
            self.alloc(node, Exp::StaticAssertLegacy(expr))
        } else {
            self.alloc(node, Exp::StaticAssert(expr))
        }
    }

    fn visibility(&mut self, node: &SyntaxNode) -> ExpId {
        let v = ast::Visibility::cast(node.clone()).expect("kind checked");
        if v.private_kw_token().is_some() {
            self.alloc(node, Exp::Private)
        } else {
            self.alloc(node, Exp::Public)
        }
    }

    /// `variable` and its flavours (`Parser.hs:449-465`), plus the
    /// `default = e;` member form (`Parser.hs:314`), which rides in `VAR_DECL`.
    fn var_decl(&mut self, node: &SyntaxNode, flag: DeclareFlag) -> ExpId {
        let decl = ast::VarDecl::cast(node.clone()).expect("kind checked");
        if decl.default_kw_token().is_some() {
            // `try $ keyword "static" *> defaultInitialization` discards the
            // `static` (`Parser.hs:475`); so does this.
            let value = match decl.expr() {
                Some(e) => self.expr(&e, false),
                None => self.unsupported(node),
            };
            return self.alloc(node, Exp::DefaultInitialization(value));
        }

        let flags = if decl.static_kw_token().is_some() && flag == DeclareFlag::DeclareLocal {
            DeclareFlag::DeclareStatic
        } else {
            flag
        };
        let wrapped = std::mem::replace(&mut self.in_template, false);
        let scope = self.qualifier();
        let attrs: Vec<ast::AttrList> = decl.attr_lists().collect();
        // A class variable's own attributes are `[[name(…)]]` alone and
        // every other variable has none (`classVariable = variable
        // DeclareClass $ optionalAttributes [strAttr Rename]`, `emptySeq`
        // for the rest, `Parser.hs:460-464`): a `[[…]]` before a function
        // type fails that `try` and is the type's (`functionType = FunctionType
        // <$> attr <*> …`, `:551`) — `[[async]] () -> void callback`
        // (`callbacks.k`'s `D`). This tree hangs it on the declaration.
        // `show Rename = "name"` (`Parser/Syntax.hs:142`).
        let renames_only = |lists: &[ast::AttrList]| {
            lists.iter().flat_map(|l| l.attrs()).all(|a| a.ident_token().is_some_and(|t| t.text() == "name"))
        };
        let to_type = !attrs.is_empty()
            && matches!(decl.ty(), Some(ast::Type::FnType(_)))
            && !(flags == DeclareFlag::DeclareClass && renames_only(&attrs));
        let attr = if to_type {
            self.fn_type_attrs = Some(attrs);
            self.synth(node, Exp::Seq(Vec::new()))
        } else {
            self.attrs(&attrs, node)
        };
        let ty = self.ty_decl_or_unsupported(decl.ty(), node);
        let name = self.identifier(decl.name(), node);
        let init = decl.expr().map(|e| self.expr(&e, false));
        let id = self.alloc(
            node,
            Exp::Variable { scope: scope.clone(), attr, ty, name, init, flags },
        );
        // `memberVariable = uninitVar DeclareMember` is *not* a `symbol`
        // (`Parser.hs:463,299`): struct and union fields never enter the
        // table. Every other flavour does (`Parser.hs:449-462`).
        if flags != DeclareFlag::DeclareMember {
            let name_sym = self.name_symbol(decl.name());
            let const_ty = matches!(self.arena[ty].exp, Exp::Const(_));
            let fn_init = init.and_then(|i| {
                matches!(self.arena[i].exp, Exp::NamedValue(_)).then_some(FnInit::Local(i))
            });
            let shape = SymShape::Variable {
                const_ty,
                type_name: Some(TypeName::Local(ty)),
                fn_init,
            };
            self.finish_decl(wrapped, name_sym, &scope, shape, id);
        }
        id
    }

    /// `staticIf p` (`Parser.hs:593-595`): a braced branch becomes a
    /// `NestedScope` via `nestedScope p` — a **Quasi** scope, unlike the
    /// **Local** of a `{}` statement — and an unbraced branch is the bare
    /// declaration or statement.
    fn static_if(&mut self, node: &SyntaxNode, branch: BranchScope) -> ExpId {
        let stmt = ast::StaticIfStmt::cast(node.clone()).expect("kind checked");
        let cond = match stmt.cond() {
            Some(c) => self.expr(&c, false),
            None => self.unsupported(node),
        };
        let (then_node, else_node) = static_if_branches(node, &stmt);
        let then_branch = match then_node {
            Some(t) => self.static_if_branch(&t, branch),
            None => self.unsupported(node),
        };
        let else_branch = else_node.map(|s| self.static_if_branch(&s, branch));
        self.alloc(node, Exp::StaticIf { cond, then_branch, else_branch })
    }

    fn static_if_branch(&mut self, node: &SyntaxNode, branch: BranchScope) -> ExpId {
        match (node.kind(), branch) {
            (SyntaxKind::BLOCK_STMT, BranchScope::Quasi) => {
                self.nested_scope(node, ScopeKind::Quasi, NestedContents::Items)
            }
            (SyntaxKind::BLOCK_STMT, BranchScope::Statement) => {
                self.nested_scope(node, ScopeKind::Quasi, NestedContents::Statements)
            }
            // An unbraced branch is whatever the enclosing position takes —
            // `p' = nestedScope p <|> p` (`Parser.hs:595`) — including item
            // kinds a `Stmt` accessor cannot see (`public:`, a declaration).
            (_, BranchScope::Quasi) => self
                .item(node)
                .unwrap_or_else(|| self.synth(node, Exp::Seq(Vec::new()))),
            (_, BranchScope::Statement) => self
                .stmt(node)
                .unwrap_or_else(|| self.synth(node, Exp::Seq(Vec::new()))),
        }
    }

    /// `nestedScope p` (`Parser.hs:467-469`): scope and `$quasiN` name captured
    /// *before* the push.
    fn nested_scope(
        &mut self,
        block: &SyntaxNode,
        kind: ScopeKind,
        contents: NestedContents,
    ) -> ExpId {
        let scope = self.qualifier();
        let index = self.scopes.get(self.scopes.current()).scope_index;
        let name_sym = self.interner.intern(&format!("{}{index}", kg_resolve::names::QUASI_PREFIX));
        let name = self.synth(block, Exp::Identifier(name_sym));

        self.push(kind, None);
        let mut children = Vec::new();
        let mut first = true;
        for child in block.children() {
            let lowered = match contents {
                NestedContents::Statements => {
                    if child.kind() == SyntaxKind::EMPTY_STMT && !first {
                        // `hidden (skipMany semi)` (`Parser.hs:479`).
                        continue;
                    }
                    first = false;
                    self.stmt(&child)
                }
                NestedContents::Items => self.item(&child),
                NestedContents::SwitchBlocks => match child.kind() {
                    SyntaxKind::SWITCH_CASE
                    | SyntaxKind::SWITCH_DEFAULT
                    | SyntaxKind::STATIC_IF_STMT => Some(self.switch_block(&child)),
                    _ => None,
                },
            };
            if let Some(id) = lowered {
                children.push(id);
            }
        }
        self.pop();

        let body = self.synth(block, Exp::Seq(children));
        self.alloc(block, Exp::NestedScope { scope, name, body })
    }

    // --- statements ---------------------------------------------------------

    /// One statement-position node, item kinds included (`statementOrDecl`,
    /// `Parser.hs:471-479`; the parser leaves local declarations as their item
    /// kinds — `DECL_STMT` is never produced).
    fn stmt(&mut self, node: &SyntaxNode) -> Option<ExpId> {
        Some(match node.kind() {
            // `hidden $ semi $> Seq []` (`Parser.hs:482`). Only the first
            // statement of a list can be one; the caller enforces position.
            SyntaxKind::EMPTY_STMT => self.alloc(node, Exp::Seq(Vec::new())),
            SyntaxKind::ERROR => self.error_node(node),
            SyntaxKind::BLOCK_STMT => {
                self.nested_scope(node, ScopeKind::Local, NestedContents::Statements)
            }
            SyntaxKind::EXPR_STMT => {
                let stmt = ast::ExprStmt::cast(node.clone()).expect("kind checked");
                match stmt.expr() {
                    Some(e) => self.expr(&e, false),
                    None => self.unsupported(node),
                }
            }
            SyntaxKind::ASSIGN_STMT => self.assign_stmt(node),
            SyntaxKind::IF_STMT => self.if_stmt(node),
            SyntaxKind::STATIC_IF_STMT => self.static_if(node, BranchScope::Statement),
            SyntaxKind::FOR_STMT => self.for_stmt(node),
            SyntaxKind::STATIC_FOR_STMT => self.static_for_stmt(node),
            SyntaxKind::DO_WHILE_STMT => self.do_while(node),
            SyntaxKind::SWITCH_STMT => self.switch(node),
            SyntaxKind::RETURN_STMT => {
                let stmt = ast::ReturnStmt::cast(node.clone()).expect("kind checked");
                let value = stmt.expr().map(|e| self.expr(&e, false));
                self.alloc(node, Exp::Return(value))
            }
            SyntaxKind::BARRIER_STMT => self.alloc(node, Exp::Barrier),
            SyntaxKind::REORDER_STMT => {
                let stmt = ast::ReorderStmt::cast(node.clone()).expect("kind checked");
                let inner = match stmt.stmt() {
                    Some(s) => self.stmt(s.syntax())?,
                    None => self.unsupported(node),
                };
                self.alloc(node, Exp::Reorder(inner))
            }
            // `Annotated (Seq [FlagAttr Atomic]) s` (`Parser.hs:559-561`).
            SyntaxKind::ATOMIC_STMT => {
                let stmt = ast::AtomicStmt::cast(node.clone()).expect("kind checked");
                let flag = self.synth(node, Exp::FlagAttr(Flag::Atomic));
                let attr = self.synth(node, Exp::Seq(vec![flag]));
                let inner = match stmt.stmt() {
                    Some(s) => self.stmt(s.syntax())?,
                    None => self.unsupported(node),
                };
                self.alloc(node, Exp::Annotated { attr, stmt: inner })
            }
            SyntaxKind::ANNOTATED_STMT => {
                let stmt = ast::AnnotatedStmt::cast(node.clone()).expect("kind checked");
                let attrs: Vec<ast::AttrList> = stmt.attr_lists().collect();
                let attr = self.attrs(&attrs, node);
                let inner = match stmt.stmt() {
                    Some(s) => self.stmt(s.syntax())?,
                    None => self.unsupported(node),
                };
                self.alloc(node, Exp::Annotated { attr, stmt: inner })
            }
            // Local declarations, plus everything `commonDecl` admits in a body.
            SyntaxKind::VAR_DECL => self.var_decl(node, DeclareFlag::DeclareLocal),
            SyntaxKind::CLASS_DECL
            | SyntaxKind::STRUCT_DECL
            | SyntaxKind::UNION_DECL
            | SyntaxKind::ENUM_DECL
            | SyntaxKind::FN_DECL
            | SyntaxKind::FN_PROTO
            | SyntaxKind::ALIAS_DECL
            | SyntaxKind::TEMPLATE_DECL
            | SyntaxKind::EXPORT_DECL
            | SyntaxKind::EXTERN_DECL
            | SyntaxKind::STATIC_ASSERT_DECL => return self.item(node),
            // A `break` outside a switch case never parses in the reference
            // (`Parser.hs:602-603`); anything else here is a construct this
            // lowering does not know.
            _ => self.unsupported(node),
        })
    }

    /// A function or lambda body: the statements are the `Seq` directly —
    /// `braces $ many statementOrDecl` (`Parser.hs:424`) — with no
    /// `NestedScope` wrapper and no extra scope beyond the caller's `Function`
    /// push.
    fn block_seq(&mut self, block: &ast::BlockStmt) -> ExpId {
        let mut out = Vec::new();
        let mut first = true;
        for child in block.syntax().children() {
            if child.kind() == SyntaxKind::EMPTY_STMT && !first {
                // `hidden (skipMany semi)` after a statement (`Parser.hs:479`).
                continue;
            }
            first = false;
            if let Some(id) = self.stmt(&child) {
                out.push(id);
            }
        }
        self.alloc(block.syntax(), Exp::Seq(out))
    }

    /// `assignStmt` and `incDecStmt` (`Parser.hs:607-638`). The lvalue is
    /// lowered twice for the desugared forms: once as written, once re-read as
    /// an rvalue — `desugar rvalue lval` (`Parser.hs:634-638`).
    fn assign_stmt(&mut self, node: &SyntaxNode) -> ExpId {
        let stmt = ast::AssignStmt::cast(node.clone()).expect("kind checked");
        let Some(lhs) = stmt.lhs() else { return self.unsupported(node) };

        let ops: Vec<SyntaxKind> = node
            .children_with_tokens()
            .filter_map(|e| e.into_token())
            .map(|t| t.kind())
            .filter(|k| !matches!(*k, SyntaxKind::SEMICOLON) && !k.is_trivia())
            .collect();

        let lvalue = self.expr(&lhs, true);

        // `++`/`--`: `Assign lval (Binary op lval 1)` (`Parser.hs:612-614`).
        if ops == [SyntaxKind::PLUS, SyntaxKind::PLUS] || ops == [SyntaxKind::MINUS, SyntaxKind::MINUS]
        {
            let op = if ops[0] == SyntaxKind::PLUS { BinOp::Add } else { BinOp::Sub };
            // The *same* `lval` node on both sides — no `desugar rvalue`
            // here, unlike `op=` — so `a[i]++` reads `a[i]` as
            // `ArrayAccessLValueF` in the sum too (`atomic.k::1`, the
            // recorded walk).
            let rval = self.reread_as(&lhs, true);
            let one = self.synth(node, Exp::IntLiteral { ty: None, value: ConstInt::ONE });
            let value = self.synth(lhs.syntax(), Exp::Binary(op, rval, one));
            return self.alloc(node, Exp::Assign { lvalue, value });
        }

        let rhs = match stmt.rhs() {
            Some(r) => self.expr(&r, false),
            None => self.unsupported(node),
        };

        let compound = match ops.as_slice() {
            [SyntaxKind::EQ] => None,
            [SyntaxKind::PLUS, SyntaxKind::EQ] => Some(BinOp::Add),
            [SyntaxKind::MINUS, SyntaxKind::EQ] => Some(BinOp::Sub),
            [SyntaxKind::STAR, SyntaxKind::EQ] => Some(BinOp::Mul),
            [SyntaxKind::SLASH, SyntaxKind::EQ] => Some(BinOp::Div),
            [SyntaxKind::PERCENT, SyntaxKind::EQ] => Some(BinOp::Mod),
            [SyntaxKind::AMP, SyntaxKind::EQ] => Some(BinOp::BitwiseAnd),
            [SyntaxKind::PIPE, SyntaxKind::EQ] => Some(BinOp::BitwiseOr),
            [SyntaxKind::CARET, SyntaxKind::EQ] => Some(BinOp::BitwiseXor),
            [SyntaxKind::AMP2, SyntaxKind::EQ] => Some(BinOp::And),
            [SyntaxKind::PIPE2, SyntaxKind::EQ] => Some(BinOp::Or),
            [SyntaxKind::CARET2, SyntaxKind::EQ] => Some(BinOp::Xor),
            _ => return self.unsupported(node),
        };

        let value = match compound {
            None => rhs,
            Some(op) => {
                let rval = self.reread(&lhs);
                self.synth(lhs.syntax(), Exp::Binary(op, rval, rhs))
            }
        };
        self.alloc(node, Exp::Assign { lvalue, value })
    }

    /// The rvalue half of a desugared assignment. The reference builds the
    /// lvalue **once** and uses that node twice — `Assign lval (Binary o lval
    /// 1)` for `++`/`--` (`Parser.hs:612-614`) and `desugar rvalue lval` for
    /// `x op= e` (`:631-638`), which rebuilds the same tree with
    /// `ArrayAccessLValueF` flattened. This port has no shared subtrees
    /// (`ExpArena::deep_copy`), so it lowers the text a second time — and the
    /// second lowering is a *copy*, not a second written node. It therefore
    /// donates: a copy claiming its donor's key is what `deep_copy` already
    /// refuses, and here it would put two `Own` nodes on one `(range, kind)`,
    /// which is the `TypeKey` an editor answers a position query from.
    ///
    /// Only the claim is dropped; the shapes are untouched. Where the two
    /// position sorters disagree — an lvalue naming a type, a `const`
    /// variable or a template parameter, `Parser.hs:794-801` against
    /// `:783-792` — this port therefore holds two *different* readings of one
    /// name where the reference holds one, and all three are assignments the
    /// reference rejects with `symbolError`. See divergence #27.
    fn reread(&mut self, lhs: &ast::Expr) -> ExpId {
        self.reread_as(lhs, false)
    }

    /// [`Self::reread`] under the lvalue reading `lv` selects.
    fn reread_as(&mut self, lhs: &ast::Expr, lv: bool) -> ExpId {
        let id = self.expr(lhs, lv);
        let mut stack = vec![id];
        let mut children = Vec::new();
        while let Some(id) = stack.pop() {
            self.arena.node_mut(id).src.written = Written::Reread;
            self.arena[id].exp.children(&mut children);
            stack.append(&mut children);
        }
        id
    }

    fn if_stmt(&mut self, node: &SyntaxNode) -> ExpId {
        let stmt = ast::IfStmt::cast(node.clone()).expect("kind checked");
        let cond = match stmt.cond() {
            Some(c) => self.expr(&c, false),
            None => self.unsupported(node),
        };
        let then_branch = match stmt.then_branch() {
            Some(t) => self.stmt(t.syntax()).unwrap_or_else(|| self.unsupported(node)),
            None => self.unsupported(node),
        };
        let else_branch = stmt
            .else_branch()
            .and_then(|e| e.stmt())
            .and_then(|s| self.stmt(s.syntax()));
        self.alloc(node, Exp::If { cond, then_branch, else_branch })
    }

    /// `rangeFor` (`Parser.hs:584`): `localScoped` wraps the head and the body
    /// both, so the induction variable's scope is already `$quasiN`.
    fn for_stmt(&mut self, node: &SyntaxNode) -> ExpId {
        let stmt = ast::ForStmt::cast(node.clone()).expect("kind checked");
        let attrs: Vec<ast::AttrList> = stmt.attr_lists().collect();
        let attr = self.attrs(&attrs, node);
        self.push(ScopeKind::Local, None);
        let var = match stmt.var_decl() {
            Some(v) => self.var_decl(v.syntax(), DeclareFlag::DeclareUninitConst),
            None => self.unsupported(node),
        };
        let limit = match stmt.limit() {
            Some(l) => self.expr(&l, false),
            None => self.unsupported(node),
        };
        let body = match stmt.body() {
            Some(b) => self.stmt(b.syntax()).unwrap_or_else(|| self.unsupported(node)),
            None => self.unsupported(node),
        };
        self.pop();
        self.alloc(node, Exp::RangeFor { attr, var, limit, body })
    }

    fn static_for_stmt(&mut self, node: &SyntaxNode) -> ExpId {
        let stmt = ast::StaticForStmt::cast(node.clone()).expect("kind checked");
        self.push(ScopeKind::Local, None);
        let var = match stmt.var_decl() {
            Some(v) => self.var_decl(v.syntax(), DeclareFlag::DeclareUninitConst),
            None => self.unsupported(node),
        };
        let limit = match stmt.limit() {
            Some(l) => self.expr(&l, false),
            None => self.unsupported(node),
        };
        let body = match stmt.body() {
            Some(b) => self.stmt(b.syntax()).unwrap_or_else(|| self.unsupported(node)),
            None => self.unsupported(node),
        };
        self.pop();
        self.alloc(node, Exp::StaticFor { var, limit, body })
    }

    /// `DoWhile attr cond stmt` — the field order swaps the parse order
    /// (`Parser.hs:586-589`).
    fn do_while(&mut self, node: &SyntaxNode) -> ExpId {
        let stmt = ast::DoWhileStmt::cast(node.clone()).expect("kind checked");
        let attrs: Vec<ast::AttrList> = stmt.attr_lists().collect();
        let attr = self.attrs(&attrs, node);
        let body = match stmt.body() {
            Some(b) => self.stmt(b.syntax()).unwrap_or_else(|| self.unsupported(node)),
            None => self.unsupported(node),
        };
        let cond = match stmt.cond() {
            Some(c) => self.expr(&c, false),
            None => self.unsupported(node),
        };
        self.alloc(node, Exp::DoWhile { attr, cond, body })
    }

    fn switch(&mut self, node: &SyntaxNode) -> ExpId {
        let stmt = ast::SwitchStmt::cast(node.clone()).expect("kind checked");
        let selector = match stmt.expr() {
            Some(e) => self.expr(&e, false),
            None => self.unsupported(node),
        };
        let mut blocks = Vec::new();
        for child in node.children() {
            match child.kind() {
                SyntaxKind::SWITCH_CASE | SyntaxKind::SWITCH_DEFAULT | SyntaxKind::STATIC_IF_STMT => {
                    blocks.push(self.switch_block(&child));
                }
                _ => {}
            }
        }
        let seq = self.synth(node, Exp::Seq(blocks));
        self.alloc(node, Exp::Switch { selector, blocks: seq })
    }

    /// One `case`/`default` block — a `Local` scope with no node of its own
    /// (`localScoped (many statementOrDecl)`, `Parser.hs:602-603`) — or a
    /// `static if` over blocks (`Parser.hs:604`), whose braced branch nests a
    /// `Seq` of blocks.
    fn switch_block(&mut self, node: &SyntaxNode) -> ExpId {
        match node.kind() {
            SyntaxKind::SWITCH_CASE => {
                let case = ast::SwitchCase::cast(node.clone()).expect("kind checked");
                let selector = case.expr();
                let value = match &selector {
                    Some(e) => self.expr(e, false),
                    None => self.unsupported(node),
                };
                self.push(ScopeKind::Local, None);
                let body = self.case_body(node, selector.as_ref().map(|e| e.syntax().clone()));
                self.pop();
                self.alloc(node, Exp::SwitchCase { value, body })
            }
            SyntaxKind::SWITCH_DEFAULT => {
                self.push(ScopeKind::Local, None);
                let body = self.case_body(node, None);
                self.pop();
                self.alloc(node, Exp::SwitchDefault(body))
            }
            SyntaxKind::STATIC_IF_STMT => {
                let stmt = ast::StaticIfStmt::cast(node.clone()).expect("kind checked");
                let cond = match stmt.cond() {
                    Some(c) => self.expr(&c, false),
                    None => self.unsupported(node),
                };
                let (then_node, else_node) = static_if_branches(node, &stmt);
                let then_branch = match then_node {
                    Some(t) if t.kind() == SyntaxKind::BLOCK_STMT => {
                        self.nested_scope(&t, ScopeKind::Quasi, NestedContents::SwitchBlocks)
                    }
                    Some(t) => self.switch_block(&t),
                    None => self.unsupported(node),
                };
                let else_branch = else_node.map(|s| {
                    if s.kind() == SyntaxKind::BLOCK_STMT {
                        self.nested_scope(&s, ScopeKind::Quasi, NestedContents::SwitchBlocks)
                    } else {
                        self.switch_block(&s)
                    }
                });
                self.alloc(node, Exp::StaticIf { cond, then_branch, else_branch })
            }
            _ => self.unsupported(node),
        }
    }

    /// The statements of a case, `break;` consumed, `Seq`-wrapped. `selector`
    /// is the case's own value expression node, skipped by identity — it can be
    /// any expression kind, so a kind test cannot tell it from a first
    /// statement.
    fn case_body(&mut self, case: &SyntaxNode, selector: Option<SyntaxNode>) -> ExpId {
        let mut out = Vec::new();
        let mut first = true;
        for child in case.children() {
            if Some(&child) == selector.as_ref() || child.kind() == SyntaxKind::BREAK_STMT {
                continue;
            }
            if child.kind() == SyntaxKind::EMPTY_STMT && !first {
                continue;
            }
            first = false;
            if let Some(id) = self.stmt(&child) {
                out.push(id);
            }
        }
        self.synth(case, Exp::Seq(out))
    }

    // --- expressions --------------------------------------------------------

    /// `lv` selects the lvalue reading: `ArrayAccessLValueF` instead of
    /// `ArrayAccessF` for `a[i]` (`Parser.hs:803-809`). Everything else is
    /// shared.
    fn expr(&mut self, e: &ast::Expr, lv: bool) -> ExpId {
        match e {
            ast::Expr::Literal(l) => self.literal(l),
            ast::Expr::InterpolatedString(s) => self.interpolated(s),
            ast::Expr::Path(p) => {
                let pos = if lv { PathPos::LValue } else { PathPos::Expr };
                self.path_pos(p, pos, false)
            }
            // `hidden (parens expr)` — no node (`Parser.hs:845`).
            ast::Expr::ParenExpr(p) => match p.expr() {
                Some(inner) => self.expr(&inner, lv),
                None => self.unsupported(p.syntax()),
            },
            ast::Expr::PrefixExpr(p) => {
                let op = match p.op_token().map(|t| t.kind()) {
                    Some(SyntaxKind::MINUS) => UnOp::Neg,
                    Some(SyntaxKind::BANG) => UnOp::Not,
                    Some(SyntaxKind::TILDE) => UnOp::Invert,
                    Some(SyntaxKind::BITSIZEOF_KW) => UnOp::BitSizeof,
                    Some(SyntaxKind::BYTESIZEOF_KW) => UnOp::ByteSizeof,
                    Some(SyntaxKind::CLOG2_KW) => UnOp::Clog2,
                    _ => return self.unsupported(p.syntax()),
                };
                let inner = match p.expr() {
                    Some(i) => self.expr(&i, false),
                    None => self.unsupported(p.syntax()),
                };
                self.alloc(p.syntax(), Exp::Unary(op, inner))
            }
            ast::Expr::BinExpr(b) => self.bin_expr(b),
            // `ternaryMux a b c = Mux a (Seq [c, b])` (`Parser.hs:968`).
            ast::Expr::TernaryExpr(t) => {
                let cond = match t.cond() {
                    Some(c) => self.expr(&c, false),
                    None => self.unsupported(t.syntax()),
                };
                let then_branch = match t.then_branch() {
                    Some(x) => self.expr(&x, false),
                    None => self.unsupported(t.syntax()),
                };
                let else_branch = match t.else_branch() {
                    Some(x) => self.expr(&x, false),
                    None => self.unsupported(t.syntax()),
                };
                let cases = self.synth(t.syntax(), Exp::Seq(vec![else_branch, then_branch]));
                self.alloc(t.syntax(), Exp::Mux { selector: cond, cases })
            }
            ast::Expr::CallExpr(c) => self.call(c),
            ast::Expr::IndexExpr(i) => {
                // `try (typ' False)` has first refusal in expression position
                // (`Parser.hs:849`): `T[4]` is an *array type* when `T`
                // denotes a type, and only otherwise an array access. This
                // CST always stores the access shape; the sort recovers the
                // reference's reading. Never in lvalue position — `lvalue`
                // has no `typ` alternative (`Parser.hs:803-805`).
                if !lv
                    && self.sorting()
                    && let Some(id) = self.index_as_array_type(i)
                {
                    return id;
                }
                let base = match i.base() {
                    Some(b) => self.expr(&b, lv),
                    None => self.unsupported(i.syntax()),
                };
                let index = match i.index() {
                    Some(x) => self.expr(&x, false),
                    None => self.unsupported(i.syntax()),
                };
                let exp = if lv {
                    Exp::ArrayAccessLValue { base, index }
                } else {
                    Exp::ArrayAccess { base, index }
                };
                self.alloc(i.syntax(), exp)
            }
            // `methodInstanceSpecifier` (`Parser.hs:766-770`), tried before
            // `value` (`:849`): `obj.template Go<uint32>` or `x.Do<uint32>`
            // outside a call is a method's instance as a value — a
            // `FunctionSpecifier` with its receiver, exactly the callee a
            // call would build (`dependent-template.k::9,10`,
            // `template-param-default.k::39`; `kg-oracle --tree --passes 0`).
            ast::Expr::FieldExpr(f)
                if !lv && field_seg_view(self.interner, f).is_some_and(|v| v.args.is_some()) =>
            {
                let receiver = f.base().map(|b| self.expr(&b, true));
                let base_key = if self.sorting() {
                    receiver.and_then(|r| self.resolve_type_name(r))
                } else {
                    None
                };
                let path = self.field_callee(f, base_key);
                self.alloc(f.syntax(), Exp::FunctionSpecifier { receiver, function: path })
            }
            ast::Expr::FieldExpr(f) => {
                let base = match f.base() {
                    Some(b) => self.expr(&b, lv),
                    None => self.unsupported(f.syntax()),
                };
                let member = self.field_member(f);
                self.alloc(f.syntax(), Exp::MemberAccess { base, member })
            }
            ast::Expr::CastExpr(c) => {
                let ty = self.ty_decl_or_unsupported(c.ty(), c.syntax());
                let value = match c.expr() {
                    Some(v) => self.expr(&v, false),
                    None => self.unsupported(c.syntax()),
                };
                self.alloc(c.syntax(), Exp::Cast { ty, value })
            }
            ast::Expr::ConcatExpr(c) => {
                let items: Vec<ExpId> = c.exprs().map(|x| self.expr(&x, false)).collect();
                let seq = self.synth(c.syntax(), Exp::Seq(items));
                self.alloc(c.syntax(), Exp::Concat(seq))
            }
            ast::Expr::MuxExpr(m) => {
                let mut items = m.exprs();
                let selector = match items.next() {
                    Some(s) => self.expr(&s, false),
                    None => self.unsupported(m.syntax()),
                };
                let cases: Vec<ExpId> = items.map(|x| self.expr(&x, false)).collect();
                let cases = self.synth(m.syntax(), Exp::Seq(cases));
                self.alloc(m.syntax(), Exp::Mux { selector, cases })
            }
            ast::Expr::FanOutExpr(f) => {
                let count = match f.count() {
                    Some(c) => self.expr(&c, false),
                    None => self.unsupported(f.syntax()),
                };
                let value = match f.value() {
                    Some(v) => self.expr(&v, false),
                    None => self.unsupported(f.syntax()),
                };
                self.alloc(f.syntax(), Exp::FanOut { count, value })
            }
            ast::Expr::StaticExpr(s) => {
                let inner = match s.expr() {
                    Some(i) => self.expr(&i, false),
                    None => self.unsupported(s.syntax()),
                };
                self.alloc(s.syntax(), Exp::Static(inner))
            }
            ast::Expr::BitOffsetExpr(b) => {
                let ty = self.ty_decl_or_unsupported(b.ty(), b.syntax());
                let field = self.name_ref_identifier(b.name_ref(), b.syntax());
                self.alloc(b.syntax(), Exp::BitOffset { ty, field })
            }
            ast::Expr::ByteOffsetExpr(b) => {
                let ty = self.ty_decl_or_unsupported(b.ty(), b.syntax());
                let field = self.name_ref_identifier(b.name_ref(), b.syntax());
                self.alloc(b.syntax(), Exp::ByteOffset { ty, field })
            }
            // `Binary LutMul` — the twentieth operator, spelled as an intrinsic
            // (`Parser.hs:866`).
            ast::Expr::LutMulExpr(l) => {
                let lhs = match l.lhs() {
                    Some(x) => self.expr(&x, false),
                    None => self.unsupported(l.syntax()),
                };
                let rhs = match l.rhs() {
                    Some(x) => self.expr(&x, false),
                    None => self.unsupported(l.syntax()),
                };
                self.alloc(l.syntax(), Exp::Binary(BinOp::LutMul, lhs, rhs))
            }
            ast::Expr::InitListExpr(i) => self.init_list(i),
            ast::Expr::LambdaExpr(l) => self.lambda(l),
            // A type written in expression position — `try (typ' False)`
            // (`Parser.hs:849`): names inside it may not be unresolved.
            ast::Expr::Type(t) => {
                let saved = std::mem::replace(&mut self.in_expr_type, true);
                let id = self.ty(t);
                self.in_expr_type = saved;
                id
            }
        }
    }

    fn bin_expr(&mut self, b: &ast::BinExpr) -> ExpId {
        let op = match b.op_token().map(|t| t.kind()) {
            Some(SyntaxKind::STAR) => BinOp::Mul,
            Some(SyntaxKind::SLASH) => BinOp::Div,
            Some(SyntaxKind::PERCENT) => BinOp::Mod,
            Some(SyntaxKind::PLUS) => BinOp::Add,
            Some(SyntaxKind::MINUS) => BinOp::Sub,
            Some(SyntaxKind::SHL) => BinOp::ShiftLeft,
            Some(SyntaxKind::SHR) => BinOp::ShiftRight,
            Some(SyntaxKind::L_ANGLE) => BinOp::Less,
            Some(SyntaxKind::R_ANGLE) => BinOp::Greater,
            Some(SyntaxKind::LTEQ) => BinOp::LessEqual,
            Some(SyntaxKind::GTEQ) => BinOp::GreaterEqual,
            Some(SyntaxKind::EQ2) => BinOp::Equal,
            Some(SyntaxKind::NEQ) => BinOp::NotEqual,
            Some(SyntaxKind::AMP) => BinOp::BitwiseAnd,
            Some(SyntaxKind::CARET) => BinOp::BitwiseXor,
            Some(SyntaxKind::PIPE) => BinOp::BitwiseOr,
            Some(SyntaxKind::AMP2) => BinOp::And,
            Some(SyntaxKind::CARET2) => BinOp::Xor,
            Some(SyntaxKind::PIPE2) => BinOp::Or,
            _ => return self.unsupported(b.syntax()),
        };
        let lhs = match b.lhs() {
            Some(l) => self.expr(&l, false),
            None => self.unsupported(b.syntax()),
        };
        let rhs = match b.rhs() {
            Some(r) => self.expr(&r, false),
            None => self.unsupported(b.syntax()),
        };
        self.alloc(b.syntax(), Exp::Binary(op, lhs, rhs))
    }

    fn literal(&mut self, l: &ast::Literal) -> ExpId {
        let node = l.syntax();
        if let Some(t) = l.int_number_token() {
            return match ConstValue::parse_int_literal(t.text()) {
                Some(ConstValue::Int { ty, value }) => {
                    self.alloc(node, Exp::IntLiteral { ty, value })
                }
                _ => self.unsupported(node),
            };
        }
        if let Some(t) = l.float_number_token() {
            return match ConstValue::parse_float_literal(t.text()) {
                Some(ConstValue::Float(bits)) => self.alloc(node, Exp::FloatLiteral(bits)),
                _ => self.unsupported(node),
            };
        }
        if l.true_kw_token().is_some() {
            return self.alloc(node, Exp::BoolLiteral(true));
        }
        if l.false_kw_token().is_some() {
            return self.alloc(node, Exp::BoolLiteral(false));
        }
        // Every string *expression* is an `InterpolatedString`, interpolations
        // or not (`Parser.hs:883`); `StringLiteralF` is only an attribute
        // argument (`Parser.hs:177,821`).
        if let Some(text) = string_text(node) {
            return self.alloc(node, Exp::InterpolatedString(text));
        }
        self.unsupported(node)
    }

    /// `InterpolatedString <$ char '"' <*> stringSegments` (`Parser.hs:883-899`):
    /// the text before each hole, and the hole — its expression, `=`, the
    /// alignment and the format. The text stays undecoded source, so a copy
    /// prints it back as written around its substituted holes.
    fn interpolated(&mut self, s: &ast::InterpolatedString) -> ExpId {
        let mut segments = Vec::new();
        let mut text = String::new();
        for element in s.syntax().children_with_tokens() {
            match element {
                kg_syntax::NodeOrToken::Token(t) if matches!(t.kind(), SyntaxKind::STRING_TEXT | SyntaxKind::STRING_END) => {
                    text.push_str(t.text());
                }
                kg_syntax::NodeOrToken::Node(n) => {
                    let Some(hole) = ast::Interpolation::cast(n.clone()) else { continue };
                    let expr = match hole.expr() {
                        Some(e) => self.expr(&e, false),
                        None => self.unsupported(&n),
                    };
                    let align = hole.align().map(|a| self.expr(&a, false));
                    let format = n
                        .children()
                        .find(|c| c.kind() == SyntaxKind::FORMAT_SPEC)
                        .map(|f| SmolStr::new(f.text().to_string()));
                    let hole = Hole { expr, echo: hole.eq_token().is_some(), align, format };
                    segments.push(Segment { text: SmolStr::new(std::mem::take(&mut text)), hole: Some(hole) });
                }
                _ => {}
            }
        }
        segments.push(Segment { text: SmolStr::new(text), hole: None });
        self.alloc(s.syntax(), Exp::Interpolated(segments.into_boxed_slice()))
    }

    fn init_list(&mut self, i: &ast::InitListExpr) -> ExpId {
        let designators: Vec<ast::Designator> = i.designators().collect();
        let items = if designators.is_empty() {
            // `Positional i <$> expression` (`Parser.hs:856-861`).
            i.exprs()
                .enumerate()
                .map(|(index, e)| {
                    let value = self.expr(&e, false);
                    self.synth(e.syntax(), Exp::Positional { index: index as u32, value })
                })
                .collect()
        } else {
            designators
                .iter()
                .map(|d| {
                    let name = self.name_ref_identifier(d.name_ref(), d.syntax());
                    let value = match d.expr() {
                        Some(e) => self.expr(&e, false),
                        None => self.unsupported(d.syntax()),
                    };
                    self.alloc(d.syntax(), Exp::Designator { name, value })
                })
                .collect()
        };
        let seq = self.synth(i.syntax(), Exp::Seq(items));
        self.alloc(i.syntax(), Exp::InitializerList(seq))
    }

    /// `lambda` (`Parser.hs:436-447`): the `?lambdaN` name is minted in the
    /// *enclosing* scope, captures and parameters live in the `Named` scope,
    /// the body under `$`; an unwritten return type is a synthesized `auto`.
    fn lambda(&mut self, l: &ast::LambdaExpr) -> ExpId {
        let node = l.syntax();
        let scope = self.qualifier();
        let name_sym = self.scopes.make_lambda_name(self.interner);
        let name = self.synth(node, Exp::Identifier(name_sym));

        self.push(ScopeKind::Named, Some(name_sym));

        // `Capture . functionScope <$> getScopeQualifier` (`Parser.hs:447`).
        let mut capture_scope = self.qualifier();
        capture_scope = capture_scope.child(self.interner.intern(kg_resolve::names::FUNCTION_SCOPE));
        let mut caps = Vec::new();
        let mut cap_syms: Vec<Symbol> = Vec::new();
        if let Some(list) = l.capture_list() {
            for cap in list.captures() {
                if self.sorting()
                    && let Some(t) = cap.name_ref().and_then(|n| n.ident_token())
                {
                    cap_syms.push(self.interner.intern(t.text()));
                }
                let cname = self.name_ref_identifier(cap.name_ref(), cap.syntax());
                caps.push(self.alloc(
                    cap.syntax(),
                    Exp::Capture { scope: capture_scope.clone(), name: cname },
                ));
            }
        }
        let captures = match l.capture_list() {
            Some(list) => self.alloc(list.syntax(), Exp::Seq(caps)),
            None => self.synth(node, Exp::Seq(caps)),
        };

        let params = self.param_list(l.param_list(), node);
        let ret = match l.ret() {
            Some(t) => self.ty_decl(&t),
            None => self.synth(node, Exp::Auto),
        };

        self.push(ScopeKind::Function, None);
        // `functionScopedWith cs` (`Parser.hs:444`): the captures are the
        // body scope's seeded symbols. A capture is a value symbol whose
        // `declScopeName` is the lambda's `$` scope.
        for sym in cap_syms {
            self.seed_symbol(sym, capture_scope.clone(), SymShape::OtherValue);
        }
        let body = match l.body() {
            Some(b) => self.block_seq(&b),
            None => self.synth(node, Exp::Seq(Vec::new())),
        };
        self.pop();
        self.pop();

        self.alloc(node, Exp::Lambda { scope, name, captures, params, ret, body })
    }

    /// A call (`Parser.hs:871`): the callee becomes a `FunctionSpecifier` — a
    /// receiver plus a name for a method (`Parser.hs:730-734`), a bare name for
    /// a free function.
    fn call(&mut self, c: &ast::CallExpr) -> ExpId {
        if let Some(chain) = self.relational_call(c) {
            return chain;
        }
        let node = c.syntax();
        let attrs: Vec<ast::AttrList> = c.attr_lists().collect();
        let attr = self.attrs(&attrs, node);

        let function = match c.callee() {
            Some(ast::Expr::FieldExpr(f)) => {
                // `methodSpecifier`'s receiver is an *lvalue* (`Parser.hs:732`).
                let receiver = f.base().map(|b| self.expr(&b, true));
                // `resolveTypeName obj` (`Parser.hs:733`) — the resolution
                // prefix the method name is looked up under.
                let base_key = if self.sorting() {
                    receiver.and_then(|r| self.resolve_type_name(r))
                } else {
                    None
                };
                let path = self.field_callee(&f, base_key);
                self.synth(f.syntax(), Exp::FunctionSpecifier { receiver, function: path })
            }
            Some(ast::Expr::Path(p)) => {
                let path = self.path_pos(&p, PathPos::Callee, false);
                self.synth(p.syntax(), Exp::FunctionSpecifier { receiver: None, function: path })
            }
            Some(other) => {
                let inner = self.expr(&other, false);
                self.synth(other.syntax(), Exp::FunctionSpecifier { receiver: None, function: inner })
            }
            None => self.unsupported(node),
        };

        let args: Vec<ExpId> = c.arg_list().map(|list| self.args(&list)).unwrap_or_default();
        let args = match c.arg_list() {
            Some(list) => self.alloc(list.syntax(), Exp::Seq(args)),
            None => self.synth(node, Exp::Seq(args)),
        };

        self.alloc(node, Exp::FunctionCall { attr, function, args })
    }

    /// A call's arguments, in the order written. One a later edition names
    /// (`x: e`, an [`ast::ArgLabel`] before its expression) is an
    /// [`Exp::Labelled`] at the label, which `kg_sem::named` puts in its
    /// parameter's place; a reference file has none, and lowers exactly
    /// the expressions [`ast::ArgList::exprs`] yields.
    fn args(&mut self, list: &ast::ArgList) -> Vec<ExpId> {
        self.labelled_items(list.syntax(), |this, e| this.expr(e, false))
    }

    /// The items of an argument list — a call's, or a template's, whose
    /// labels (edition 1.1's named template arguments) `kg_sem::deduce`
    /// places — each lowered by `arg`, one a label names wrapped in an
    /// [`Exp::Labelled`] at the label.
    fn labelled_items(&mut self, list: &SyntaxNode, mut arg: impl FnMut(&mut Self, &ast::Expr) -> ExpId) -> Vec<ExpId> {
        let mut out = Vec::new();
        let mut label: Option<ast::ArgLabel> = None;
        for child in list.children() {
            if let Some(l) = ast::ArgLabel::cast(child.clone()) {
                label = Some(l);
            } else if let Some(e) = ast::Expr::cast(child) {
                let value = arg(self, &e);
                let named = label.take().and_then(|l| Some((l.ident_token()?, l)));
                out.push(match named {
                    Some((ident, l)) => {
                        let label = self.interner.intern(ident.text());
                        self.alloc(l.syntax(), Exp::Labelled { label, value })
                    }
                    None => value,
                });
            }
        }
        out
    }

    /// `Name<E>(C)` read as the reference's parser reads it when `Name` is
    /// no template at that point: `findSymbol` decides whether template
    /// arguments are parsed at all (`Parser.hs:669-680`), and without them
    /// the `<` is an operator, so the call is the chain `Name < E > (C)`
    /// (`dependent-template.k::16`: `Do` is the `else` branch's variable
    /// there, and `Do<a && b>(10)` is `(Do < a) && (b > 10)`).
    ///
    /// Inside the angles `E` was parsed without `<`, `>`, `<<`, `>>` at its
    /// top (`opsTemplate`, `Parser.hs:913`); every operator it can hold
    /// there binds looser than `<` and `>` except the arithmetic ones and
    /// `<=`/`>=` (`Parser.hs:921-958`). So `Name <` takes `E`'s leftmost
    /// operand below its looser operators and `> (C)` its rightmost; an
    /// `E` with no looser operator at its top is `Name < E > C`, which
    /// `no_assoc` rejects, and so is an outer operand that is `<=`/`>=`.
    /// Only where the call stands alone — no operator or postfix around it
    /// that could take `Name` or `C` first — is the reading made here;
    /// everything else keeps the committed tree (the walk declines it).
    fn relational_call(&mut self, c: &ast::CallExpr) -> Option<ExpId> {
        if !self.sorting() || c.attr_lists().next().is_some() {
            return None;
        }
        let parent = c.syntax().parent()?.kind();
        if matches!(
            parent,
            SyntaxKind::BIN_EXPR
                | SyntaxKind::PREFIX_EXPR
                | SyntaxKind::TERNARY_EXPR
                | SyntaxKind::INDEX_EXPR
                | SyntaxKind::FIELD_EXPR
                | SyntaxKind::CALL_EXPR
                | SyntaxKind::CAST_EXPR
        ) {
            return None;
        }
        let Some(ast::Expr::Path(p)) = c.callee() else { return None };
        let segs: Vec<ast::PathSegment> = p.path_segments().collect();
        let [seg] = segs.as_slice() else { return None };
        let view = seg_view(self.interner, seg);
        let list = view.args.clone()?;
        if view.template_kw {
            return None;
        }
        let args: Vec<ast::Expr> = list.exprs().collect();
        let [e @ ast::Expr::BinExpr(_)] = args.as_slice() else { return None };
        let calls: Vec<ast::Expr> = c.arg_list()?.exprs().collect();
        let [arg] = calls.as_slice() else { return None };

        // `prefixedScopedNameAndSymbol`'s test (`Parser.hs:669-680`): the
        // arguments are parsed only for a template, or a template-template
        // parameter (`:661`).
        let scope_hit = {
            let tab = self.sort.as_ref().expect("sorting");
            tab.find(&self.arena, self.interner, &self.scopes, &[], view.name)
        };
        let templated = scope_hit.as_ref().is_some_and(|h| match self.sort.as_ref().expect("sorting").shape(h) {
            SymShape::TemplateParam(param) => param.kind_is_template,
            SymShape::Alias { target: Some(_) } => false,
            shape => shape.is_template(),
        });
        if templated {
            return None;
        }

        let rhs = self.expr(e, false);
        if !is_looser_than_relational(&self.arena[rhs].exp) {
            return None;
        }
        let bare = SegView { args: None, ..view };
        let name = match self.sort_segments(p.syntax(), &[bare], PathPos::Expr) {
            Ok(id) => id,
            Err(_) => return None,
        };
        // The name alone is the operand, and the path's text still holds
        // the `<…>` it no longer has: what is headed there is `Donated`, as
        // a node whose head is not its own text is.
        let path_range = p.syntax().text_range();
        let mut stack = vec![name];
        while let Some(id) = stack.pop() {
            let node = self.arena.node_mut(id);
            if node.src.head.range == path_range {
                node.src.written = kg_base::Written::Donated;
            }
            let mut kids = Vec::new();
            self.arena[id].exp.children(&mut kids);
            stack.extend(kids);
        }
        let operand = self.expr(arg, false);
        let donor = c.syntax().clone();
        let left = self.attach_operand(rhs, true, name, &donor)?;
        let right = self.attach_operand(left, false, operand, &donor)?;
        Some(right)
    }

    /// `relational_call`'s re-association: `name < …` at the leftmost
    /// operand below `id`'s looser operators (`left`), or `… > operand` at
    /// the rightmost. The spine is rewritten in place — those nodes were
    /// lowered a moment ago for this reading alone.
    fn attach_operand(&mut self, id: ExpId, left: bool, with: ExpId, donor: &SyntaxNode) -> Option<ExpId> {
        if let Exp::Binary(op, l, r) = self.arena[id].exp.clone()
            && is_looser_than_relational(&self.arena[id].exp)
        {
            let exp = if left {
                Exp::Binary(op, self.attach_operand(l, true, with, donor)?, r)
            } else {
                Exp::Binary(op, l, self.attach_operand(r, false, with, donor)?)
            };
            self.arena.node_mut(id).exp = exp;
            return Some(id);
        }
        // `<=`/`>=` share the level with `<`/`>`, `no_assoc`: the
        // reference's parse fails there.
        if matches!(self.arena[id].exp, Exp::Binary(BinOp::LessEqual | BinOp::GreaterEqual, ..)) {
            return None;
        }
        Some(if left {
            self.synth(donor, Exp::Binary(BinOp::Less, with, id))
        } else {
            self.synth(donor, Exp::Binary(BinOp::Greater, id, with))
        })
    }

    /// The member half of a `FIELD_EXPR`. A plain member is the reference's
    /// `MemberAccessF _ (Identifier m)` (`Parser.hs:814-818`); an instantiated
    /// one is a single-segment path carrying its written arguments.
    fn field_member(&mut self, f: &ast::FieldExpr) -> ExpId {
        let Some(view) = field_seg_view(self.interner, f) else {
            return self.unsupported(f.syntax());
        };
        if view.args.is_none() && !view.template_kw {
            return self.alloc(&view.donor, Exp::Identifier(view.name));
        }
        // `.member<args>` as a value is `methodInstanceSpecifier`'s, lowered
        // before this (`Expr::FieldExpr`'s first arm); what reaches here —
        // an lvalue's (`notFollowedBy templateArgs`, `Parser.hs:812`), a
        // `.template` with no arguments — has no sorted shape.
        if self.sorting() {
            self.count_unsorted(&view.donor, UnsortedReason::ArgsOnNonTemplate);
        }
        self.view_path(&view)
    }

    /// The callee half of a method call: always a name-shaped path, the
    /// analogue of the `QualifiedIdentifier` a `methodSpecifier` builds
    /// (`Parser.hs:730-734`).
    fn field_callee(&mut self, f: &ast::FieldExpr, base_key: Option<SymbolPath>) -> ExpId {
        let Some(view) = field_seg_view(self.interner, f) else {
            return self.unsupported(f.syntax());
        };
        if self.sorting() {
            let donor = view.donor.clone();
            match self.sort_segments(&donor, std::slice::from_ref(&view), PathPos::Method {
                base_key,
            }) {
                Ok(id) => return id,
                Err(reason) => self.count_unsorted(&donor, reason),
            }
        }
        self.view_path(&view)
    }

    /// A single-segment unsorted path from one written segment.
    fn view_path(&mut self, view: &SegView) -> ExpId {
        let args = view.args.as_ref().map(|list| {
            let items: Vec<ExpId> = list.exprs().map(|e| self.expr(&e, false)).collect();
            self.alloc(list.syntax(), Exp::Seq(items))
        });
        let segment = PathSeg { template_kw: view.template_kw, name: view.name, args };
        self.alloc(
            &view.donor,
            Exp::Path(UnsortedPath { typename_kw: false, segments: vec![segment], refused: None }),
        )
    }

    fn path_seg(&mut self, seg: &ast::PathSegment) -> PathSeg {
        let name = seg
            .name_ref()
            .and_then(|n| n.ident_token())
            .map(|t| self.interner.intern(t.text()))
            .unwrap_or_else(|| self.interner.intern(""));
        let args = seg.template_arg_list().map(|list| {
            let items = self.labelled_items(list.syntax(), |this, e| this.expr(e, false));
            self.alloc(list.syntax(), Exp::Seq(items))
        });
        PathSeg { template_kw: seg.template_kw_token().is_some(), name, args }
    }

    fn path(&mut self, p: &ast::Path, typename_kw: bool, refused: Option<Refused>) -> ExpId {
        let segments: Vec<PathSeg> = p.path_segments().map(|s| self.path_seg(&s)).collect();
        self.alloc(p.syntax(), Exp::Path(UnsortedPath { typename_kw, segments, refused }))
    }

    // --- types --------------------------------------------------------------

    fn ty_or_unsupported(&mut self, t: Option<ast::Type>, donor: &SyntaxNode) -> ExpId {
        match t {
            Some(t) => self.ty(&t),
            None => self.synth(donor, Exp::Unsupported),
        }
    }

    fn ty(&mut self, t: &ast::Type) -> ExpId {
        match t {
            ast::Type::VoidType(v) => self.alloc(v.syntax(), Exp::Void),
            ast::Type::BoolType(b) => self.alloc(b.syntax(), Exp::Boolean),
            ast::Type::FloatType(f) => self.alloc(f.syntax(), Exp::Float),
            ast::Type::StringType(s) => self.alloc(s.syntax(), Exp::Str),
            ast::Type::AutoType(a) => self.alloc(a.syntax(), Exp::Auto),
            // `keyword "typename" *> typeSpecifier' True` (`Parser.hs:547`):
            // unresolved names allowed even inside `typ' False`. Unsorted,
            // the keyword survives as a flag on the path.
            ast::Type::TypenameType(t) => match t.path_type().and_then(|p| p.path()) {
                Some(p) => self.path_pos(&p, PathPos::Ty { allow_unresolved: true }, true),
                None => self.unsupported(t.syntax()),
            },
            // `int32` is one token carrying its width (`Lexer.hs:241-243`).
            ast::Type::IntType(i) => {
                if let Some(t) = i.uint_type_token() {
                    match t.text().strip_prefix("uint").and_then(|w| w.parse().ok()) {
                        Some(w) => self.alloc(i.syntax(), Exp::Unsigned(w)),
                        None => self.unsupported(i.syntax()),
                    }
                } else if let Some(t) = i.int_type_token() {
                    match t.text().strip_prefix("int").and_then(|w| w.parse().ok()) {
                        Some(w) => self.alloc(i.syntax(), Exp::Integer(w)),
                        None => self.unsupported(i.syntax()),
                    }
                } else {
                    self.unsupported(i.syntax())
                }
            }
            ast::Type::ParamIntType(p) => {
                let width = match p.expr() {
                    Some(e) => self.expr(&e, false),
                    None => self.unsupported(p.syntax()),
                };
                if p.uint_kw_token().is_some() {
                    self.alloc(p.syntax(), Exp::ParamUint(width))
                } else {
                    self.alloc(p.syntax(), Exp::ParamInt(width))
                }
            }
            ast::Type::ConstType(c) => {
                let inner = self.ty_or_unsupported(c.ty(), c.syntax());
                self.alloc(c.syntax(), Exp::Const(inner))
            }
            // `Array attr elem (Seq dims)` (`Parser.hs:529-538`).
            ast::Type::ArrayType(a) => {
                let attrs: Vec<ast::AttrList> = a.attr_lists().collect();
                let attr = self.attrs(&attrs, a.syntax());
                let elem = self.ty_or_unsupported(a.elem(), a.syntax());
                let dims: Vec<ExpId> = a.exprs().map(|e| self.expr(&e, false)).collect();
                let dims = self.synth(a.syntax(), Exp::Seq(dims));
                self.alloc(a.syntax(), Exp::Array { attr, elem, dims })
            }
            ast::Type::PathType(p) => match p.path() {
                Some(path) => {
                    let allow_unresolved = !self.in_expr_type;
                    self.path_pos(&path, PathPos::Ty { allow_unresolved }, false)
                }
                None => self.unsupported(p.syntax()),
            },
            // `functionType` (`Parser.hs:551-557`): the parameter list is a
            // `Quasi` scope; the return type is parsed after the pop.
            ast::Type::FnType(f) => {
                let mut attrs: Vec<ast::AttrList> = self.fn_type_attrs.take().unwrap_or_default();
                attrs.extend(f.attr_lists());
                let attr = self.attrs(&attrs, f.syntax());
                self.push(ScopeKind::Quasi, None);
                let mut params = Vec::new();
                if let Some(list) = f.fn_type_param_list() {
                    for param in list.fn_type_params() {
                        let pattrs: Vec<ast::AttrList> = param.attr_lists().collect();
                        let pattr = self.attrs(&pattrs, param.syntax());
                        let pty = self.ty_or_unsupported(param.ty(), param.syntax());
                        let pname = param.name().map(|n| {
                            let sym = self.interner.intern(
                                n.ident_token().map(|t| t.text().to_owned()).unwrap_or_default().as_str(),
                            );
                            self.alloc(n.syntax(), Exp::Identifier(sym))
                        });
                        params.push(self.alloc(
                            param.syntax(),
                            Exp::FunctionTypeParam { attr: pattr, ty: pty, name: pname },
                        ));
                    }
                }
                self.pop();
                let params = match f.fn_type_param_list() {
                    Some(list) => self.alloc(list.syntax(), Exp::Seq(params)),
                    None => self.synth(f.syntax(), Exp::Seq(params)),
                };
                let ret = self.ty_or_unsupported(f.ret(), f.syntax());
                self.alloc(f.syntax(), Exp::FunctionType { attr, params, ret })
            }
            ast::Type::DecltypeType(d) => {
                let scope = self.qualifier();
                let inner = match d.expr() {
                    Some(e) => self.expr(&e, false),
                    None => self.unsupported(d.syntax()),
                };
                self.alloc(d.syntax(), Exp::Decltype { scope, expr: inner })
            }
        }
    }

    // --- attributes ---------------------------------------------------------

    /// `[[a]] [[b]]` and `[[a, b]]` are the same one `Seq` (`Parser.hs:163-165`);
    /// no attributes at all is `Seq []` (`emptySeq`, `Parser.hs:167`).
    fn attrs(&mut self, lists: &[ast::AttrList], donor: &SyntaxNode) -> ExpId {
        let mut items = Vec::new();
        for list in lists {
            for attr in list.attrs() {
                items.push(self.attr(&attr));
            }
        }
        match lists {
            // One written group: the `Seq` *is* that `[[…]]`.
            [only] => self.alloc(only.syntax(), Exp::Seq(items)),
            // Several groups merge into one `Seq` with no node of its own.
            [first, ..] => {
                let donor = first.syntax().clone();
                self.synth(&donor, Exp::Seq(items))
            }
            [] => self.synth(donor, Exp::Seq(items)),
        }
    }

    /// One written attribute, sorted into the reference's three shapes by its
    /// name (`Parser.hs:171-177`). An unknown name is a parse error there and
    /// an `Unsupported` here, so gate 1 counts it instead of guessing.
    fn attr(&mut self, attr: &ast::Attr) -> ExpId {
        let node = attr.syntax();
        let Some(name) = attr.ident_token() else { return self.unsupported(node) };
        let arg = attr.attr_args().and_then(|a| a.expr());

        if let Some(flag) = flag_by_name(name.text()) {
            if attr.attr_args().is_some() {
                return self.unsupported(node);
            }
            return self.alloc(node, Exp::FlagAttr(flag));
        }
        if let Some(int_attr) = int_attr_by_name(name.text()) {
            let value = match arg {
                Some(e) => self.expr(&e, false),
                None => return self.unsupported(node),
            };
            return self.alloc(node, Exp::IntAttr(int_attr, value));
        }
        match name.text() {
            // `identAttr Ecc` (`Parser.hs:175,510`).
            "ecc" => {
                let value = match arg {
                    Some(ast::Expr::Path(p)) if self.sorting() => {
                        self.path_pos(&p, PathPos::IdentAttr, false)
                    }
                    Some(e) => self.expr(&e, false),
                    None => return self.unsupported(node),
                };
                self.alloc(node, Exp::Attr(Attribute::Ecc, value))
            }
            // `strAttr Rename` (`Parser.hs:177`): the argument is a
            // `StringLiteralF`, the one place that node exists.
            "name" => {
                let value = match arg.as_ref().map(|e| e.syntax().clone()) {
                    Some(arg_node) => match string_text(&arg_node) {
                        Some(text) => self.alloc(&arg_node, Exp::StringLiteral(text)),
                        None => return self.unsupported(node),
                    },
                    None => return self.unsupported(node),
                };
                self.alloc(node, Exp::Attr(Attribute::Rename, value))
            }
            _ => self.unsupported(node),
        }
    }

    fn name_ref_identifier(&mut self, name: Option<ast::NameRef>, donor: &SyntaxNode) -> ExpId {
        match name {
            Some(n) => {
                let sym = n
                    .ident_token()
                    .map(|t| self.interner.intern(t.text()))
                    .unwrap_or_else(|| self.interner.intern(""));
                self.alloc(n.syntax(), Exp::Identifier(sym))
            }
            None => {
                let sym = self.interner.intern("");
                self.synth(donor, Exp::Identifier(sym))
            }
        }
    }

    // --- path sorting (`Parser.hs:640-801`) ---------------------------------
    //
    // Everything below runs only when `self.sort` is `Some`. The shapes are
    // pinned by the oracle probes in the test module: each helper cites the
    // reference line it transcribes.

    /// Whether paths sort at all this lowering.
    fn sorting(&self) -> bool {
        self.sort.is_some()
    }

    /// `parseSymbol` (`SymbolTable.hs:194-214`): scope + module map. `decl`
    /// is the completed node the entry stands for — what `nub` compares.
    fn declare_now(
        &mut self,
        name: Symbol,
        decl_scope: &QualifiedName,
        shape: SymShape,
        decl: ExpId,
    ) {
        let Some(tab) = self.sort.as_mut() else { return };
        let id = tab.push(SymData {
            name,
            decl_scope: decl_scope.clone(),
            shape,
            decl: Some(decl),
        });
        tab.file_symbol(&mut self.scopes, id);
    }

    /// A declaration's `symbol` wrapper resolved: a declaration wrapped by a
    /// `template <…>` reports back instead of declaring — the reference
    /// declares only the `Template` node, under the inner name
    /// (`Parser.hs:373-375`).
    fn finish_decl(
        &mut self,
        wrapped_in_template: bool,
        name: Symbol,
        decl_scope: &QualifiedName,
        shape: SymShape,
        decl: ExpId,
    ) {
        if self.sort.is_none() {
            return;
        }
        if wrapped_in_template {
            self.pending_decl = Some((name, shape));
        } else {
            self.declare_now(name, decl_scope, shape, decl);
        }
    }

    /// `parseScoped`'s seeding (`SymbolTable.hs:188`): into the innermost
    /// scope, bypassing the module map and the redefinition check.
    fn seed_symbol(&mut self, name: Symbol, decl_scope: QualifiedName, shape: SymShape) -> Option<ItemId> {
        let tab = self.sort.as_mut()?;
        let id = tab.push(SymData { name, decl_scope, shape, decl: None });
        self.scopes.seed(name, id);
        Some(id)
    }

    /// Re-seed an already-built entry (a template parameter travelling from
    /// the parameter list into the wrapped declaration's scope).
    fn reseed(&mut self, id: ItemId) {
        let Some(tab) = self.sort.as_ref() else { return };
        let name = tab.get(id).name;
        self.scopes.seed(name, id);
    }

    /// A type written in a *declaration* position — every place the reference
    /// re-enters `typ` (= `typ' True`, `Parser.hs:515`), so unresolved names
    /// are allowed again even inside an expression-position type (a `cast`
    /// inside an array dimension, for instance).
    fn ty_decl(&mut self, t: &ast::Type) -> ExpId {
        let saved = std::mem::replace(&mut self.in_expr_type, false);
        let id = self.ty(t);
        self.in_expr_type = saved;
        id
    }

    fn ty_decl_or_unsupported(&mut self, t: Option<ast::Type>, donor: &SyntaxNode) -> ExpId {
        match t {
            Some(t) => self.ty_decl(&t),
            None => self.synth(donor, Exp::Unsupported),
        }
    }

    /// One path, sorted if a symbol table is present, unsorted otherwise.
    fn path_pos(&mut self, p: &ast::Path, pos: PathPos, typename_kw: bool) -> ExpId {
        if !self.sorting() {
            return self.path(p, typename_kw, None);
        }
        let segs: Vec<SegView> =
            p.path_segments().map(|s| seg_view(self.interner, &s)).collect();
        match self.sort_segments(p.syntax(), &segs, pos) {
            Ok(id) => id,
            Err(reason) => {
                self.count_unsorted(p.syntax(), reason);
                // The head's own lookup, asked again as `sort_segments`
                // asked it first: `findSymbol` with no qualifier
                // (`Parser.hs:657`).
                let head_found = segs.first().is_none_or(|seg| {
                    let tab = self.sort.as_ref().expect("sorting");
                    tab.find(&self.arena, self.interner, &self.scopes, &[], seg.name).is_some()
                });
                let in_type = p.syntax().parent().is_some_and(|x| x.kind() == SyntaxKind::PATH_TYPE);
                let refused = Refused { scope: self.qualifier(), head_found, in_type };
                self.path(p, typename_kw, Some(refused))
            }
        }
    }

    fn count_unsorted(&mut self, node: &SyntaxNode, reason: UnsortedReason) {
        let range = FileRange::new(self.file, node.text_range());
        if let Some(tab) = self.sort.as_mut() {
            tab.stats.unsorted.push((range, reason));
        }
    }

    /// The spine walk — `prefixedScopedNameAndSymbol` (`Parser.hs:652-695`) —
    /// then the position's sorter. `Err` is the honest refusal: the written
    /// tree has no reference shape to copy, so the caller falls back to the
    /// unsorted [`Exp::Path`] and the reason is counted.
    #[allow(clippy::too_many_lines)]
    fn sort_segments(
        &mut self,
        donor: &SyntaxNode,
        segs: &[SegView],
        pos: PathPos,
    ) -> Result<ExpId, UnsortedReason> {
        // `getScopeQualifier` at the head of the name (`Parser.hs:649`); every
        // `ScopedNameF` of this path carries it.
        let scope = self.qualifier();

        // `methodSpecifier` starts pre-qualified: the written prefix is the
        // `$unresolved` placeholder, the resolution prefix is the receiver's
        // type, and no qualifier is ever materialised (`Parser.hs:730-734,764`).
        let unresolved = self.interner.intern(crate::sort::UNRESOLVED_NAME);
        let (mut prefix, mut lookup_base, mut is_qualified) = match &pos {
            PathPos::Method { base_key } => {
                let ident = self.synth(donor, Exp::Identifier(unresolved));
                let placeholder = self.synth(
                    donor,
                    Exp::ScopedName { scope: QualifiedName::empty(), prefix: None, ident },
                );
                let base = base_key.clone().unwrap_or_else(|| SymbolPath::single(unresolved));
                (Some(placeholder), base, true)
            }
            _ => (None, SymbolPath::empty(), false),
        };

        let mut last: Option<Hit> = None;
        for (i, seg) in segs.iter().enumerate() {
            let force_template = seg.template_kw;
            let ident = match &seg.name_node {
                Some(n) => self.alloc(n, Exp::Identifier(seg.name)),
                None => self.synth(&seg.donor, Exp::Identifier(seg.name)),
            };
            let s = self.synth(
                &seg.donor,
                Exp::ScopedName { scope: scope.clone(), prefix, ident },
            );

            // `findSymbol (ScopedNameF scope resolvedPrefix ident)`
            // (`Parser.hs:657`): qualifier = the resolution prefix's qualified
            // name, empty for an unqualified head.
            let qualifier: Vec<Symbol> = if i == 0 && !matches!(pos, PathPos::Method { .. }) {
                Vec::new()
            } else {
                lookup_base.segments().to_vec()
            };
            let hit = {
                let tab = self.sort.as_ref().expect("sort_segments runs only when sorting");
                tab.find(&self.arena, self.interner, &self.scopes, &qualifier, seg.name)
            };

            let shape = hit.as_ref().map(|h| {
                self.sort.as_ref().expect("checked").shape(h).clone()
            });
            match shape {
                // `Just e@(TemplateParam n a b c _)` (`Parser.hs:658-666`):
                // the segment *becomes* `TypeParamF n a b c`, never qualified.
                Some(SymShape::TemplateParam(param)) => {
                    let kind = self.arena.deep_copy(param.kind);
                    let mut node = self.synth(
                        &seg.donor,
                        Exp::TypeParam {
                            scope: param.scope.clone(),
                            kind,
                            name: seg.name,
                            index: param.index,
                        },
                    );
                    if param.kind_is_template {
                        node = self.template_instance(node, seg, false)?;
                    } else if seg.args.is_some() {
                        // The reference would not parse an argument list here
                        // (`templateInstance` is not consulted), leaving the
                        // `<…>` text unconsumed.
                        return Err(UnsortedReason::ArgsOnNonTemplate);
                    }
                    prefix = Some(node);
                    lookup_base = param.scope.child(seg.name);
                    is_qualified = true;
                }
                // `Just e@(Alias _ (TypeIdentifier _ t) _)` (`Parser.hs:667-668`):
                // the resolution prefix becomes the alias's target; the
                // written spine keeps its spelling.
                Some(SymShape::Alias { target: Some(t) }) if !force_template => {
                    if seg.args.is_some() {
                        return Err(UnsortedReason::ArgsOnNonTemplate);
                    }
                    let emitted = if is_qualified {
                        s
                    } else {
                        let ds = self.sort_decl_scope(hit.as_ref().expect("shape implies hit"));
                        self.set_qualifier(s, ds.segments(), &seg.donor);
                        s
                    };
                    prefix = Some(emitted);
                    lookup_base = t;
                    is_qualified = true;
                }
                // `Just e | forceTemplate || isTemplate e` (`Parser.hs:669-672`).
                Some(sh) if force_template || sh.is_template() => {
                    let inst = self.template_instance(s, seg, sh.is_function())?;
                    // `resolvedPrefix' = i` binds the *unqualified* instance
                    // (`Parser.hs:671-672`) — capture its name before the
                    // qualifier is grafted in.
                    lookup_base = SymbolPath::new(self.spine_qualname(inst));
                    if !is_qualified {
                        let ds = self.sort_decl_scope(hit.as_ref().expect("shape implies hit"));
                        self.set_qualifier(inst, ds.segments(), &seg.donor);
                    }
                    prefix = Some(inst);
                    is_qualified = true;
                }
                // `Just e | otherwise` (`Parser.hs:673-674`).
                Some(_) => {
                    if seg.args.is_some() {
                        return Err(UnsortedReason::ArgsOnNonTemplate);
                    }
                    lookup_base = SymbolPath::new(self.spine_qualname(s));
                    if !is_qualified {
                        let ds = self.sort_decl_scope(hit.as_ref().expect("shape implies hit"));
                        self.set_qualifier(s, ds.segments(), &seg.donor);
                    }
                    prefix = Some(s);
                    is_qualified = true;
                }
                // `Nothing | forceTemplate` (`Parser.hs:676-678`): the written
                // `template` keyword forces an instance even unresolved;
                // `isFunction` of a bare spine is false, so no `Seq []`.
                None if force_template => {
                    let inst = self.template_instance(s, seg, false)?;
                    lookup_base = SymbolPath::new(self.spine_qualname(inst));
                    prefix = Some(inst);
                    is_qualified = true;
                }
                // `Nothing` (`Parser.hs:679-680`).
                None => {
                    if seg.args.is_some() {
                        return Err(UnsortedReason::ArgsOnNonTemplate);
                    }
                    lookup_base = SymbolPath::new(self.spine_qualname(s));
                    prefix = Some(s);
                }
            }
            last = hit;
        }

        let spine = prefix.expect("paths have at least one segment");
        self.finish_sorted(donor, spine, last.as_ref(), &scope, pos)
    }

    /// The position sorters — `typeSpecifier'` (`Parser.hs:712-726`),
    /// `templateSpecifier` (`:702-708`), `namedValue` (`:783-792`),
    /// `namedLValue` (`:794-801`), `freeFunctionSpecifier` (`:772-780`),
    /// `methodSpecifier` (`:730-734`).
    #[allow(clippy::too_many_lines)]
    fn finish_sorted(
        &mut self,
        donor: &SyntaxNode,
        spine: ExpId,
        last: Option<&Hit>,
        scope: &QualifiedName,
        pos: PathPos,
    ) -> Result<ExpId, UnsortedReason> {
        let shape = last.map(|h| self.sort.as_ref().expect("sorting").shape(h).clone());
        let is_instance = matches!(self.arena[spine].exp, Exp::TemplateInstance { .. });

        // The outermost sorted node claims the CST slot the unsorted `Path`
        // used to own; a spine returned bare is re-stamped the same way. Any
        // node *inside* it claiming the same slot donates — see
        // [`Lower::donate_inner_claims`].
        let own = |lo: &mut Self, id: ExpId| {
            let src = lo.own_src(donor);
            let node = lo.arena.node_mut(id);
            node.src = src;
            node.kind = donor.kind();
            lo.donate_inner_claims(id, src.head, donor.kind());
            id
        };

        match pos {
            PathPos::Ty { allow_unresolved } => match shape {
                // `Just t@TemplateParam{} | isType t && not (isTemplate t)`
                // (`Parser.hs:716-718`).
                Some(SymShape::TemplateParam(p)) if !p.kind_is_type && !p.kind_is_template => {
                    Ok(own(self, spine))
                }
                // `symbolError s "template arguments missing"` returns the
                // spine (`Parser.hs:720-721`, `Error.hs:136-137`); kg-hir
                // diagnoses nothing, so only the shape is reproduced.
                Some(sh) if sh.is_template() && !is_instance => Ok(own(self, spine)),
                Some(sh) if sh.is_type() => {
                    Ok(self.alloc_at(donor, Exp::TypeIdentifier(scope.clone(), spine)))
                }
                // A value symbol here fails the reference's parse
                // (`unexpectedSymbol`, `Parser.hs:723`) and reparses down a
                // grammar path this CST did not take — no shape to copy.
                Some(_) => Err(UnsortedReason::ValueInTypePosition),
                None if allow_unresolved => {
                    Ok(self.alloc_at(donor, Exp::TypeIdentifier(scope.clone(), spine)))
                }
                None => Err(UnsortedReason::UnresolvedNestedTypeInExpr),
            },
            PathPos::Expr | PathPos::TemplateArg => {
                // `try templateSpecifier` first for a template argument
                // (`Parser.hs:697,702-708`).
                if matches!(pos, PathPos::TemplateArg)
                    && shape.as_ref().is_some_and(|sh| sh.is_template())
                    && !is_instance
                {
                    return Ok(self.alloc_at(donor, Exp::TypeIdentifier(scope.clone(), spine)));
                }
                match shape {
                    // The `try (typ' False)` route (`Parser.hs:849,716-718`).
                    Some(SymShape::TemplateParam(p)) if !p.kind_is_type && !p.kind_is_template => {
                        Ok(own(self, spine))
                    }
                    // `namedValue`'s parameter cases (`Parser.hs:788-791`) —
                    // the single-segment spine *is* the `TypeParamF`, so only
                    // the `FunctionType` wrapper differs.
                    Some(SymShape::TemplateParam(p)) if p.kind_is_function_type => {
                        Ok(self.alloc_at(donor, Exp::NamedValue(spine)))
                    }
                    Some(SymShape::TemplateParam(_)) => Ok(own(self, spine)),
                    // `typeSpecifier' False`'s delayed-error arm returns the
                    // spine (`Parser.hs:720-721`).
                    Some(sh) if sh.is_template() && !is_instance => Ok(own(self, spine)),
                    Some(sh) if sh.is_type() => {
                        Ok(self.alloc_at(donor, Exp::TypeIdentifier(scope.clone(), spine)))
                    }
                    // `namedValue`'s catch-all: `NamedValue s` where `s` is
                    // the `QualifiedIdentifier`-wrapped spine (`Parser.hs:792`,
                    // `qualifiedIdentifierAndSymbol` at `:642-645`).
                    _ => {
                        let qi = self.synth(donor, Exp::QualifiedIdentifier(spine));
                        Ok(self.alloc_at(donor, Exp::NamedValue(qi)))
                    }
                }
            }
            PathPos::LValue => match shape {
                // Both `symbolError` arms return `s` — the bare
                // `QualifiedIdentifier` (`Parser.hs:797-800`).
                Some(sh) if sh.is_type() => {
                    let qi = self.synth(donor, Exp::QualifiedIdentifier(spine));
                    Ok(own(self, qi))
                }
                Some(SymShape::Variable { const_ty: true, .. }) => {
                    let qi = self.synth(donor, Exp::QualifiedIdentifier(spine));
                    Ok(own(self, qi))
                }
                _ => {
                    let qi = self.synth(donor, Exp::QualifiedIdentifier(spine));
                    Ok(self.alloc_at(donor, Exp::NamedValue(qi)))
                }
            },
            PathPos::Callee => match shape {
                // `Just (TemplateParam n a@FunctionType{} b c _)` — the bare
                // `TypeParam`, no wrapper (`Parser.hs:774-775`).
                Some(SymShape::TemplateParam(p)) if p.kind_is_function_type => {
                    Ok(own(self, spine))
                }
                // Any other parameter: `symbolError s …` returns the
                // `QualifiedIdentifier` (`Parser.hs:778-779`).
                Some(SymShape::TemplateParam(_)) => {
                    let qi = self.synth(donor, Exp::QualifiedIdentifier(spine));
                    Ok(own(self, qi))
                }
                // The const-function substitution (`Parser.hs:776-777`): the
                // initializer subtree replaces the callee.
                Some(SymShape::Variable { const_ty: true, fn_init: Some(init), .. }) => match init {
                    FnInit::Local(id) => Ok(self.arena.deep_copy(id)),
                    FnInit::Foreign => Err(UnsortedReason::ForeignConstFnInit),
                },
                _ => {
                    let qi = self.synth(donor, Exp::QualifiedIdentifier(spine));
                    Ok(own(self, qi))
                }
            },
            PathPos::Method { .. } => {
                // `QualifiedIdentifier . fst <$> prefixedScopedNameAndSymbol …`
                // (`Parser.hs:734`).
                let qi = self.synth(donor, Exp::QualifiedIdentifier(spine));
                Ok(own(self, qi))
            }
            PathPos::IdentAttr => {
                let qi = self.synth(donor, Exp::QualifiedIdentifier(spine));
                Ok(own(self, qi))
            }
            PathPos::ExportTy => match shape {
                // `try templateSpecifier` (`Parser.hs:278,280,702-708`).
                Some(sh) if sh.is_template() && !is_instance => {
                    Ok(self.alloc_at(donor, Exp::TypeIdentifier(scope.clone(), spine)))
                }
                _ => self.finish_sorted(donor, spine, last, scope, PathPos::Ty {
                    allow_unresolved: true,
                }),
            },
        }
    }

    /// An `Own` node at `donor` — the sorted replacement for the `Own`
    /// `Exp::Path` the unsorted lowering allocates at the same slot.
    fn alloc_at(&mut self, donor: &SyntaxNode, exp: Exp) -> ExpId {
        self.alloc(donor, exp)
    }

    /// Every node inside `outer` that claims `outer`'s own `(head, kind)`
    /// gives the claim up: **the outermost sorted node keeps it**.
    ///
    /// `(head, kind)` is a `TypeKey` (`kg-ty/src/table.rs`) and
    /// `written == Own` is what makes one legal, so two claimants are two
    /// answers to one position question — and the outermost node is the one
    /// such a question should be answered with. A method callee is where the
    /// sorted lowering produces the pair: [`Lower::field_callee`] hands the
    /// whole path the `NAME_REF` *itself* as its donor, so the leaf
    /// `Identifier` built from that `NAME_REF` and the `QualifiedIdentifier`
    /// wrapped round it land on one key. Over `library/` that is 201 keys in
    /// 30 modules, every one an ancestor/descendant pair at
    /// `Expansion::ROOT`.
    ///
    /// **The shapes are right and are not touched.** The reference hands the
    /// same nested nodes one identical note — `let s = copyNote (ScopedNameF
    /// scope prefix ident) ident` (`Parser.hs:655`) and `return (copyNote
    /// (QualifiedIdentifierF s) s, sym)` (`:644`) — and `Written` is this
    /// port's own field, which the reference has no analogue of. So the
    /// invariant moved, not the tree; divergence #24 is the history.
    ///
    /// This is the *enclosure* case only. One CST slot is also claimed twice
    /// from two **sibling** spines — the compound assignment's lvalue re-read,
    /// 114 keys over `library/` and 72 of them in `lower` as well — and that
    /// one is [`Lower::reread`]'s, for the same reason and with the same
    /// answer.
    fn donate_inner_claims(&mut self, outer: ExpId, head: FileRange, kind: SyntaxKind) {
        let mut stack = Vec::new();
        let mut children = Vec::new();
        self.arena[outer].exp.children(&mut stack);
        while let Some(id) = stack.pop() {
            let node = self.arena.node_mut(id);
            if node.src.written == Written::Own && node.src.head == head && node.kind == kind {
                node.src.written = Written::Donated;
            }
            self.arena[id].exp.children(&mut children);
            stack.append(&mut children);
        }
    }

    /// `templateInstance` (`Parser.hs:690-695`): written arguments wrap the
    /// spine; a function symbol with no written list still becomes an
    /// instance over `Seq []`; anything else passes through.
    fn template_instance(
        &mut self,
        base: ExpId,
        seg: &SegView,
        sym_is_function: bool,
    ) -> Result<ExpId, UnsortedReason> {
        match &seg.args {
            Some(list) => {
                let items = self.labelled_items(list.syntax(), Self::template_arg);
                let args = self.alloc(list.syntax(), Exp::Seq(items));
                Ok(self.synth(&seg.donor, Exp::TemplateInstance { base, args }))
            }
            None if sym_is_function => {
                let args = self.synth(&seg.donor, Exp::Seq(Vec::new()));
                Ok(self.synth(&seg.donor, Exp::TemplateInstance { base, args }))
            }
            None => Ok(base),
        }
    }

    /// One written template argument — `try templateSpecifier <|>
    /// expressionTemplate` (`Parser.hs:697`).
    fn template_arg(&mut self, e: &ast::Expr) -> ExpId {
        match e {
            ast::Expr::Path(p) if self.sorting() => {
                self.path_pos(p, PathPos::TemplateArg, false)
            }
            _ => self.expr(e, false),
        }
    }

    /// `declScopeName` of a resolved symbol — what `makeQualified`
    /// materialises (`Desugar.hs:188-191`).
    fn sort_decl_scope(&self, hit: &Hit) -> SymbolPath {
        self.sort.as_ref().expect("sorting").decl_scope(hit).clone()
    }

    /// `setQualifier` (`Desugar.hs:193-201`), grafting in place: a written
    /// prefix whose name matches the qualifier's last component absorbs it
    /// and recurses; otherwise the prefix is replaced by the materialised
    /// chain.
    fn set_qualifier(&mut self, id: ExpId, qualifier: &[Symbol], donor: &SyntaxNode) {
        if qualifier.is_empty() {
            return;
        }
        match self.arena[id].exp.clone() {
            Exp::ScopedName { prefix: Some(a), .. }
                if self.spine_name(a) == qualifier.last().copied() =>
            {
                self.set_qualifier(a, &qualifier[..qualifier.len() - 1], donor);
            }
            Exp::ScopedName { scope, .. } => {
                let chain = self.scoped_qualifier(&scope, qualifier, donor);
                if let Exp::ScopedName { prefix, .. } = &mut self.arena.node_mut(id).exp {
                    *prefix = chain;
                }
            }
            Exp::TemplateInstance { base, .. } | Exp::QualifiedIdentifier(base) => {
                self.set_qualifier(base, qualifier, donor);
            }
            _ => {}
        }
    }

    /// `scopedQualifier` (`Desugar.hs:182-186`): the qualifier's components as
    /// a chain of prefix `ScopedName`s, innermost first.
    fn scoped_qualifier(
        &mut self,
        scope: &QualifiedName,
        qualifier: &[Symbol],
        donor: &SyntaxNode,
    ) -> Option<ExpId> {
        let mut prev: Option<ExpId> = None;
        for &component in qualifier {
            let ident = self.synth(donor, Exp::Identifier(component));
            prev = Some(self.synth(
                donor,
                Exp::ScopedName { scope: scope.clone(), prefix: prev, ident },
            ));
        }
        prev
    }

    /// `getQualifiedName` over a built spine (`Parser/Syntax.hs:673,744-763`).
    fn spine_qualname(&self, id: ExpId) -> Vec<Symbol> {
        match &self.arena[id].exp {
            Exp::ScopedName { prefix, ident, .. } => {
                let mut out = prefix.map(|p| self.spine_qualname(p)).unwrap_or_default();
                out.extend(self.spine_name(*ident));
                out
            }
            Exp::Identifier(s) => vec![*s],
            Exp::TypeParam { scope, name, .. } => {
                let mut out: Vec<Symbol> = scope.segments().to_vec();
                out.push(*name);
                out
            }
            Exp::TemplateInstance { base, .. }
            | Exp::QualifiedIdentifier(base)
            | Exp::NamedValue(base)
            | Exp::TypeIdentifier(_, base) => self.spine_qualname(*base),
            _ => Vec::new(),
        }
    }

    /// `getName` over a built spine.
    fn spine_name(&self, id: ExpId) -> Option<Symbol> {
        match &self.arena[id].exp {
            Exp::Identifier(s) => Some(*s),
            Exp::ScopedName { ident, .. } => self.spine_name(*ident),
            Exp::TypeParam { name, .. } => Some(*name),
            Exp::TemplateInstance { base, .. }
            | Exp::QualifiedIdentifier(base)
            | Exp::NamedValue(base)
            | Exp::TypeIdentifier(_, base) => self.spine_name(*base),
            _ => None,
        }
    }

    // --- the receiver's type (`resolveTypeName`, `Parser.hs:736-762`) -------

    /// The resolution prefix a method name is looked up under. `None` is the
    /// reference's `unresolved` — the `$unresolved` placeholder that resolves
    /// nothing.
    fn resolve_type_name(&self, id: ExpId) -> Option<SymbolPath> {
        match &self.arena[id].exp {
            Exp::ArrayAccessLValue { base, .. } => self.resolve_type_name(*base),
            Exp::NamedValue(x) | Exp::QualifiedIdentifier(x) => self.resolve_type_name(*x),
            Exp::MemberAccess { base, member } => {
                let t = self.resolve_type_name(*base);
                let name = self.spine_name(*member)?;
                self.resolve_scoped(t, name)
            }
            Exp::ScopedName { .. } => {
                let qual = self.spine_qualname(id);
                let (&name, init) = qual.split_last()?;
                self.resolve_scoped_key(init, name)
            }
            _ => None,
        }
    }

    /// `resolveTypeName s@(ScopedName _ _ x)` (`Parser.hs:744-751`): the
    /// spine's own lookup, then the bare name as a fallback.
    fn resolve_scoped_key(&self, qualifier: &[Symbol], name: Symbol) -> Option<SymbolPath> {
        let tab = self.sort.as_ref()?;
        match tab.find(&self.arena, self.interner, &self.scopes, qualifier, name) {
            Some(hit) => match tab.shape(&hit) {
                SymShape::Variable { type_name: Some(t), .. } => self.type_key(t),
                _ => None,
            },
            None => {
                // `findSymbol x` — the bare identifier (`Parser.hs:746-748`).
                match tab.find(&self.arena, self.interner, &self.scopes, &[], name) {
                    Some(hit) => match tab.shape(&hit) {
                        SymShape::Variable { type_name: Some(t), .. } => self.type_key(t),
                        _ => None,
                    },
                    None => None,
                }
            }
        }
    }

    /// The `MemberAccess` leg (`Parser.hs:740-743`): the member resolved
    /// under the base's type name.
    fn resolve_scoped(&self, base: Option<SymbolPath>, name: Symbol) -> Option<SymbolPath> {
        let unresolved = self.interner.intern(crate::sort::UNRESOLVED_NAME);
        let base = base.unwrap_or_else(|| SymbolPath::single(unresolved));
        self.resolve_scoped_key(base.segments(), name)
    }

    /// `typeName` (`Parser.hs:752-757`): unwrap arrays, then resolve the
    /// named type, chasing alias bodies. Anything else — `const`, built-ins,
    /// parameters — is unresolved.
    fn type_key(&self, t: &TypeName) -> Option<SymbolPath> {
        let key = match t {
            TypeName::Key(k) => k.clone(),
            &TypeName::Local(id) => {
                let mut id = id;
                loop {
                    match &self.arena[id].exp {
                        Exp::Array { elem, .. } => id = *elem,
                        Exp::TypeIdentifier(_, payload) => {
                            break SymbolPath::new(self.spine_qualname(*payload));
                        }
                        _ => return None,
                    }
                }
            }
        };
        self.chase_type_key(key)
    }

    /// The `findSymbol`-and-alias-chase half of `typeName`
    /// (`Parser.hs:753-756`). Bounded: an alias cycle is an error program.
    fn chase_type_key(&self, mut key: SymbolPath) -> Option<SymbolPath> {
        let tab = self.sort.as_ref()?;
        for _ in 0..8 {
            let segments = key.segments().to_vec();
            let (&name, qualifier) = segments.split_last()?;
            match tab.find(&self.arena, self.interner, &self.scopes, qualifier, name) {
                Some(hit) => match tab.shape(&hit) {
                    SymShape::Alias { target: Some(t) } => key = t.clone(),
                    _ => return Some(key),
                },
                None => return None,
            }
        }
        None
    }

    // --- declaration facts for the table ------------------------------------

    /// The written type of a class-member seed, resolved *now* — the state
    /// the reference's `lookAhead` pre-parse resolves it in (`Parser.hs:317-321`).
    fn written_type_key(&self, t: &ast::Type) -> Option<SymbolPath> {
        match t {
            ast::Type::PathType(p) => self.written_path_key(&p.path()?),
            ast::Type::TypenameType(t) => {
                self.written_path_key(&t.path_type().and_then(|p| p.path())?)
            }
            _ => None,
        }
    }

    /// A written path's qualified name after the sort *would* qualify it —
    /// enough for a seed's lookup key, without building nodes.
    fn written_path_key(&self, p: &ast::Path) -> Option<SymbolPath> {
        let tab = self.sort.as_ref()?;
        let mut key: Vec<Symbol> = Vec::new();
        let mut resolved = false;
        for seg in p.path_segments() {
            let name = self.interner.intern(
                seg.name_ref().and_then(|n| n.ident_token()).map(|t| t.text().to_owned())?.as_str(),
            );
            let hit = tab.find(&self.arena, self.interner, &self.scopes, &key, name);
            if let Some(h) = hit {
                match tab.shape(&h) {
                    SymShape::Alias { target: Some(t) } => {
                        key = t.segments().to_vec();
                        resolved = true;
                        continue;
                    }
                    SymShape::TemplateParam(param) => {
                        key = param.scope.segments().to_vec();
                        key.push(name);
                        resolved = true;
                        continue;
                    }
                    _ => {
                        if !resolved {
                            key = tab.decl_scope(&h).segments().to_vec();
                        }
                        resolved = true;
                    }
                }
            }
            key.push(name);
        }
        Some(SymbolPath::new(key))
    }

    /// The lookahead pre-parse of a class body (`class_`, `Parser.hs:315-321`):
    /// the class itself plus every member that is a template, a type or a
    /// variable — never a member function — seeded into the `unscopedWith`
    /// wrapper so members resolve each other regardless of order.
    fn preseed_class(
        &mut self,
        wrapped_in_template: bool,
        class_name: Symbol,
        class_scope: &QualifiedName,
        list: Option<&ast::DeclList>,
    ) {
        if self.sort.is_none() {
            return;
        }
        // `classTemplate` seeds the `Template` node, `class_` the `Class`
        // (`Parser.hs:319,327`).
        let self_shape = if wrapped_in_template {
            SymShape::Template { function: false, type_like: true }
        } else {
            SymShape::Type
        };
        self.seed_symbol(class_name, class_scope.clone(), self_shape);

        let member_scope = class_scope.child(class_name);
        let Some(list) = list else { return };
        for child in list.syntax().children() {
            let (name_node, shape) = match child.kind() {
                SyntaxKind::STRUCT_DECL
                | SyntaxKind::UNION_DECL
                | SyntaxKind::ENUM_DECL
                | SyntaxKind::CLASS_DECL => (child.clone(), SymShape::Type),
                SyntaxKind::ALIAS_DECL => {
                    let target = ast::AliasDecl::cast(child.clone())
                        .and_then(|d| d.ty())
                        .and_then(|t| self.written_type_key(&t));
                    (child.clone(), SymShape::Alias { target })
                }
                SyntaxKind::VAR_DECL => {
                    let decl = ast::VarDecl::cast(child.clone()).expect("kind checked");
                    if decl.default_kw_token().is_some() {
                        continue;
                    }
                    let const_ty = decl
                        .ty()
                        .is_some_and(|t| matches!(t, ast::Type::ConstType(_)));
                    let type_name = decl
                        .ty()
                        .and_then(|t| self.member_type_key(&t))
                        .map(TypeName::Key);
                    (
                        child.clone(),
                        SymShape::Variable { const_ty, type_name, fn_init: None },
                    )
                }
                SyntaxKind::TEMPLATE_DECL => {
                    let decl = ast::TemplateDecl::cast(child.clone()).expect("kind checked");
                    let Some(inner) = decl.decl() else { continue };
                    let inner_kind = inner.syntax().kind();
                    let shape = SymShape::Template {
                        function: matches!(
                            inner_kind,
                            SyntaxKind::FN_DECL | SyntaxKind::FN_PROTO
                        ),
                        type_like: matches!(
                            inner_kind,
                            SyntaxKind::STRUCT_DECL
                                | SyntaxKind::UNION_DECL
                                | SyntaxKind::CLASS_DECL
                                | SyntaxKind::ENUM_DECL
                                | SyntaxKind::ALIAS_DECL
                        ),
                    };
                    (inner.syntax().clone(), shape)
                }
                _ => continue,
            };
            if let Some(name) = decl_name(&name_node) {
                let sym = self.interner.intern(&name);
                self.seed_symbol(sym, member_scope.clone(), shape);
            }
        }
    }

    /// A member variable's type key for its seed — `typeName`'s array unwrap
    /// applied to the written form (`Parser.hs:752`).
    fn member_type_key(&self, t: &ast::Type) -> Option<SymbolPath> {
        match t {
            ast::Type::ArrayType(a) => self.member_type_key(&a.elem()?),
            _ => self.written_type_key(t),
        }
    }
}

/// The syntactic position a written path is sorted for — which of the
/// reference's name parsers would have consumed it.
#[derive(Clone, Debug)]
enum PathPos {
    /// `typeSpecifier' allowUnresolved` (`Parser.hs:712-726`). `True` in every
    /// declaration position (`typ`), `False` inside an expression
    /// (`try (typ' False)`, `Parser.hs:849`).
    Ty { allow_unresolved: bool },
    /// An expression term (`value` → `namedValue`, `Parser.hs:783-792`, after
    /// `try (typ' False)` has had first refusal).
    Expr,
    /// An assignment target (`namedLValue`, `Parser.hs:794-801`).
    LValue,
    /// A free-function callee (`freeFunctionSpecifier`, `Parser.hs:772-780`).
    Callee,
    /// A method callee: written prefix `$unresolved`, resolution prefix the
    /// receiver's type name (`methodSpecifier`, `Parser.hs:730-734`).
    Method { base_key: Option<SymbolPath> },
    /// A written template argument (`try templateSpecifier <|>
    /// expressionTemplate`, `Parser.hs:697`).
    TemplateArg,
    /// `export` / `extern` (`try templateSpecifier <|> typ`, `Parser.hs:278,280`).
    ExportTy,
    /// An `identAttr` argument (`parens qualifiedIdentifier`, `Parser.hs:175`):
    /// the spine wrapped in `QualifiedIdentifierF` and nothing else — no
    /// position sorter consults the symbol's shape (`:640-644`).
    IdentAttr,
}

/// An operator of a level below `<` and `>` in the reference's table —
/// equality, the bitwise and the logical ones (`Parser.hs:939-955`); the
/// ternary sits below them too but is a `Mux`, not a `Binary`, here.
fn is_looser_than_relational(exp: &Exp) -> bool {
    matches!(
        exp,
        Exp::Binary(
            BinOp::Equal
                | BinOp::NotEqual
                | BinOp::BitwiseAnd
                | BinOp::BitwiseXor
                | BinOp::BitwiseOr
                | BinOp::And
                | BinOp::Xor
                | BinOp::Or,
            ..
        )
    )
}

/// One written segment, uniform across `ast::Path` segments and the
/// `PathSegment`/bare-`NameRef` shapes a method callee arrives in.
struct SegView {
    name: Symbol,
    name_node: Option<SyntaxNode>,
    template_kw: bool,
    args: Option<ast::TemplateArgList>,
    donor: SyntaxNode,
}

fn seg_view(interner: &Interner, seg: &ast::PathSegment) -> SegView {
    let name_ref = seg.name_ref();
    SegView {
        name: name_ref
            .as_ref()
            .and_then(|n| n.ident_token())
            .map(|t| interner.intern(t.text()))
            .unwrap_or_else(|| interner.intern("")),
        name_node: name_ref.map(|n| n.syntax().clone()),
        template_kw: seg.template_kw_token().is_some(),
        args: seg.template_arg_list(),
        donor: seg.syntax().clone(),
    }
}

/// The member half of a `FIELD_EXPR` as one written segment. The parser
/// writes it as loose children — `DOT`, optional `TEMPLATE_KW`, a `NAME_REF`,
/// an optional `TEMPLATE_ARG_LIST` (`grammar/exprs.rs:269-279`) — never as a
/// `PATH_SEGMENT`, so `FieldExpr::path_segment()` finds nothing here.
fn field_seg_view(interner: &Interner, f: &ast::FieldExpr) -> Option<SegView> {
    let name_ref = f.syntax().children().find(|c| c.kind() == SyntaxKind::NAME_REF)?;
    let template_kw = f
        .syntax()
        .children_with_tokens()
        .filter_map(|e| e.into_token())
        .any(|t| t.kind() == SyntaxKind::TEMPLATE_KW);
    let args = f.syntax().children().find_map(ast::TemplateArgList::cast);
    Some(SegView {
        name: name_ref
            .first_token()
            .map(|t| interner.intern(t.text()))
            .unwrap_or_else(|| interner.intern("")),
        name_node: Some(name_ref.clone()),
        template_kw,
        args,
        donor: name_ref,
    })
}

/// The name a declaration node declares — its `NAME` child's identifier.
fn decl_name(node: &SyntaxNode) -> Option<String> {
    node.children().find(|c| c.kind() == SyntaxKind::NAME).and_then(|name| {
        name.children_with_tokens()
            .filter_map(|e| e.into_token())
            .find(|t| t.kind() == SyntaxKind::IDENT)
            .map(|t| t.text().to_owned())
    })
}

/// Which reading a braced `static if` branch gets: items at declaration level
/// (`staticIf decl` / `staticIf memberDecl`), statements inside a body
/// (`staticIf statementOrDecl`). Both are `nestedScope`, i.e. `Quasi`
/// (`Parser.hs:593-595`).
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
enum BranchScope {
    Quasi,
    Statement,
}

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
enum NestedContents {
    Statements,
    Items,
    SwitchBlocks,
}

/// The 12 writable flag names (`Parser.hs:171` with the call sites at
/// `:384-397,505-511,581`). `atomic` is deliberately absent: `FlagAttr Atomic`
/// exists only as the desugaring of the `atomic` statement (`Parser.hs:559-561`).
fn flag_by_name(name: &str) -> Option<Flag> {
    Some(match name {
        "async" => Flag::Async,
        "last" => Flag::EndTransaction,
        "initialize" => Flag::Initialize,
        "memory" => Flag::Memory,
        "no_backpressure" => Flag::NoBackPressure,
        "pure" => Flag::Pure,
        "non_replicated" => Flag::NonReplicated,
        "pipelined" => Flag::Pipelined,
        "quad_port" => Flag::QuadPort,
        "reorder_by_looping" => Flag::ReorderByLooping,
        "reset" => Flag::Reset,
        "unordered" => Flag::Unordered,
        _ => return None,
    })
}

/// The seven `intAttr`s (`Parser.hs:173` with call sites).
fn int_attr_by_name(name: &str) -> Option<Attribute> {
    Some(match name {
        "call_rate" => Attribute::CallRate,
        "fifo_depth" => Attribute::FifoDepth,
        "latency" => Attribute::Latency,
        "max_threads" => Attribute::MaxThreads,
        "schedule" => Attribute::Schedule,
        "thread_rate" => Attribute::ThreadRate,
        "transaction_size" => Attribute::TransactionSize,
        _ => return None,
    })
}

/// The two branch *nodes* of a `static if`, found positionally: the typed
/// `then_branch()`/`ElseBranch::stmt()` accessors are `Option<Stmt>` and come
/// back `None` whenever a branch is an item kind — `static if (X) public:`,
/// an unbraced declaration, or a bare `case` inside a `switch` — because the
/// parser never wraps those in a `DECL_STMT`.
fn static_if_branches(
    node: &SyntaxNode,
    stmt: &ast::StaticIfStmt,
) -> (Option<SyntaxNode>, Option<SyntaxNode>) {
    let cond = stmt.cond().map(|c| c.syntax().clone());
    let then_branch = node
        .children()
        .find(|c| Some(c) != cond.as_ref() && c.kind() != SyntaxKind::ELSE_BRANCH);
    let else_branch = node
        .children()
        .find(|c| c.kind() == SyntaxKind::ELSE_BRANCH)
        .and_then(|e| e.children().next());
    (then_branch, else_branch)
}

/// The variants of an `enum`, read off the tree the parser actually builds.
///
/// The generated `EnumDecl::enum_variants` looks among the declaration's
/// *direct* children, but the parser wraps the body in a `DECL_LIST`, so the
/// accessor comes back empty on every real enum — the defect
/// `kg-db/src/facts.rs:125-140` documents. That helper is `pub(crate)` in a
/// crate that will sit *above* this one, so the six lines are repeated here
/// with the same citation rather than inverting the crate order for them.
fn enum_variants(decl: &ast::EnumDecl) -> impl Iterator<Item = ast::EnumVariant> + use<> {
    decl.syntax().descendants().filter_map(ast::EnumVariant::cast)
}

/// The `T` of `enum E : T` — mandatory (`Parser.hs:284`), and the only `Type`
/// among the declaration's direct children. See [`enum_variants`].
fn enum_base_type(decl: &ast::EnumDecl) -> Option<ast::Type> {
    decl.syntax().children().find_map(ast::Type::cast)
}

/// The text of a node that is exactly one string literal: its `STRING`
/// token, or — an attribute's argument, where `stringLiteral` reads a `{` as
/// text (`Parser/Lexer.hs:161-162`) — the whole `INTERPOLATED_STRING` the
/// lexer split it into.
fn string_text(node: &SyntaxNode) -> Option<SmolStr> {
    node.children_with_tokens()
        .find_map(|e| match e {
            kg_syntax::NodeOrToken::Token(t) if t.kind() == SyntaxKind::STRING => Some(SmolStr::new(t.text())),
            kg_syntax::NodeOrToken::Node(n) if n.kind() == SyntaxKind::INTERPOLATED_STRING => {
                Some(SmolStr::new(n.text().to_string()))
            }
            _ => None,
        })
        .or_else(|| {
            node.children()
                .find(|c| c.kind() == SyntaxKind::LITERAL)
                .and_then(|c| string_text(&c))
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use kg_base::Written;

    fn lower_str(text: &str) -> (crate::ModuleIr, Interner) {
        let interner = Interner::new();
        let parse = kg_syntax::parse_file(text);
        let file = ast::SourceFile::cast(parse.syntax_node()).expect("root");
        let ir = lower(&file, FileId(0), &interner);
        (ir, interner)
    }

    fn structure(text: &str) -> String {
        let (ir, interner) = lower_str(text);
        ir.structure_string(&interner)
    }

    /// A class callback's leading `[[…]]` is its function type's
    /// (`functionType`, `Parser.hs:551`) unless it is the variable's own
    /// `[[name(…)]]` (`classVariable`, `:464`): the walk hands the backend
    /// the variable's attributes and the type's apart (`callbacks.k::0`,
    /// the recorded walk). Red with the list left on the variable.
    #[test]
    fn a_callbacks_leading_attributes_are_its_function_types() {
        let (ir, _) = lower_str("class D\n{\n    [[async]] () -> void callback;\n    [[name(\"x\")]] () -> void named;\n}\n");
        let variables: Vec<(usize, usize)> = ir
            .arena
            .iter()
            .filter_map(|(_, n)| match &n.exp {
                Exp::Variable { attr, ty, .. } => {
                    let own = match &ir.arena[*attr].exp {
                        Exp::Seq(items) => items.len(),
                        _ => usize::MAX,
                    };
                    let typed = match &ir.arena[*ty].exp {
                        Exp::FunctionType { attr, .. } => match &ir.arena[*attr].exp {
                            Exp::Seq(items) => items.len(),
                            _ => usize::MAX,
                        },
                        _ => usize::MAX,
                    };
                    Some((own, typed))
                }
                _ => None,
            })
            .collect();
        assert_eq!(variables, [(0, 1), (1, 0)]);
    }

    /// `ternaryMux a b c = Mux a $ Seq [c, b]` (`Parser.hs:968`) — the arms
    /// are *reversed*, which is the single easiest thing to port wrong.
    #[test]
    fn a_ternary_lowers_to_a_mux_with_reversed_arms() {
        let s = structure("inline void f()\n{\n    uint8 x = true ? 1 : 2;\n}\n");
        let mux = s.find("Mux").expect("a ternary lowers to Mux");
        let tail = &s[mux..];
        let one = tail.find("IntLiteral 1").expect("then arm present");
        let two = tail.find("IntLiteral 2").expect("else arm present");
        assert!(two < one, "the else arm comes first (`Seq [c, b]`):\n{s}");
    }

    /// `assign' s o` re-reads the written lvalue as an rvalue —
    /// `desugar rvalue lval` turns `ArrayAccessLValueF` into `ArrayAccessF`
    /// (`Parser.hs:631-638`).
    #[test]
    fn a_compound_assignment_reads_its_lvalue_twice() {
        let s = structure("inline void f()\n{\n    uint8[4] a;\n    a[0] += 1;\n}\n");
        assert!(s.contains("Assign"), "{s}");
        assert!(s.contains("ArrayAccessLValue"), "the written lvalue reading:\n{s}");
        assert!(s.contains("Binary Add"), "the desugared operator:\n{s}");
        let rv = s.find("Binary Add").expect("checked");
        assert!(s[rv..].contains("ArrayAccess\n") || s[rv..].contains("ArrayAccess "),
                "the rvalue re-read drops the LValue flavour:\n{s}");
    }

    /// `obj.template Go<uint32>` as a value is `methodInstanceSpecifier`'s
    /// `FunctionSpecifier (Just obj) …` (`Parser.hs:765-769`): `access`
    /// never reads a member followed by template arguments (`:812`). A
    /// member without them stays a `MemberAccess`. Red with the arm taken
    /// out (`dependent-template.k::10` was an unsorted path).
    #[test]
    fn a_member_template_instance_named_as_a_value_is_a_method_specifier() {
        let s = structure("class B\n{\npublic:\n    template <typename T>\n    T Go()\n    {\n        return {};\n    }\n    uint32 x;\n}\n\ninline void f()\n{\n    static B obj;\n    const () -> uint32 g = obj.template Go<uint32>;\n    uint32 y = obj.x;\n}\n");
        let g = s.find("FunctionSpecifier").unwrap_or_else(|| panic!("no FunctionSpecifier:\n{s}"));
        assert!(s[..g].contains("Variable"), "{s}");
        assert!(s.contains("MemberAccess"), "`obj.x` stays a member access:\n{s}");
        assert_eq!(s.matches("FunctionSpecifier").count(), 1, "{s}");
    }

    /// `incDecStmt` hands the *same* `lval` to both sides — `Assign lval
    /// (Binary o lval 1)`, no `desugar rvalue` (`Parser.hs:612-614`) — so the
    /// sum reads `a[0]` as `ArrayAccessLValueF` too, unlike `op=`
    /// (`atomic.k::1`, the recorded walk's `ParseAccessArrayLValue`).
    #[test]
    fn an_increment_reads_its_lvalue_twice_as_an_lvalue() {
        let s = structure("inline void f()\n{\n    uint8[4] a;\n    a[0]++;\n}\n");
        let rv = s.find("Binary Add").expect("the desugared operator");
        assert!(s[rv..].contains("ArrayAccessLValue"), "the sum keeps the lvalue reading:\n{s}");
    }

    /// …and the re-read *claims* nothing. The reference builds the lvalue
    /// once and hands the same node to both positions (`Parser.hs:612-614`,
    /// `:631-638`); a second lowering of the same text is this port's copy of
    /// it. Two `Own` nodes on one `(range, kind)` would be two answers to one
    /// position query.
    ///
    /// It is [`Written::Reread`], not `Donated`, since 2026-09-20. The copy's
    /// `head` **is** its own text — the reference stamps both readings with
    /// the same type and gives both the same span (`static-if.k::17`'s `x++`,
    /// the recorded walk) — and only the key is another node's. `Donated` said
    /// both things at once and the walk, which reads a type only where a
    /// position is answerable, gave the second reading neither.
    #[test]
    fn the_lvalue_re_read_of_a_compound_assignment_is_its_own_text() {
        let (ir, _) = lower_str("inline void f()\n{\n    uint8 x = 0;\n    x += 1;\n}\n");
        let (_, assign) = ir
            .arena
            .iter()
            .find(|(_, n)| matches!(n.exp, Exp::Assign { .. }))
            .expect("the compound assignment lowers to Assign");
        let Exp::Assign { lvalue, value } = assign.exp else { unreachable!("matched above") };
        let Exp::Binary(_, rval, _) = ir.arena[value].exp else {
            panic!("`x op= e` is `Assign x (Binary op x' e)`")
        };
        assert_eq!(ir.arena[rval].src.head, ir.arena[lvalue].src.head, "same written text");
        assert_eq!(ir.arena[rval].kind, ir.arena[lvalue].kind);
        assert_eq!(
            ir.arena[lvalue].src.written,
            Written::Own,
            "the written lvalue keeps the claim"
        );
        assert_eq!(
            ir.arena[rval].src.written,
            Written::Reread,
            "the rvalue re-read is its own text and claims nothing"
        );
    }

    /// `x++` is `Assign x (Binary Add x 1)` (`Parser.hs:607-614`), and the
    /// synthesized `1` wears a donor's range, never its own.
    #[test]
    fn increment_desugars_to_assign_plus_one_with_a_donated_literal() {
        let (ir, interner) = lower_str("inline void f()\n{\n    uint8 x = 0;\n    x++;\n}\n");
        let s = ir.structure_string(&interner);
        assert!(s.contains("Binary Add"), "{s}");
        assert!(s.contains("IntLiteral 1"), "{s}");
        let one = ir
            .arena
            .iter()
            .find(|(_, n)| matches!(&n.exp, Exp::IntLiteral { ty: None, value } if *value == ConstInt::ONE))
            .expect("the synthesized 1 exists");
        assert_eq!(one.1.src.written, Written::Donated, "a synthesized node is never Own");
    }

    /// `makeSymbolName "?lambda"` numbers lambdas per scope
    /// (`Parser.hs:434`, `SymbolTable.hs:216-224`), and an unwritten return
    /// type is a synthesized `auto` (`Parser.hs:443`).
    #[test]
    fn lambdas_are_numbered_in_scope_order_and_default_to_auto() {
        let s = structure(
            "inline void f()\n{\n    auto a = [] { return 1; };\n    auto b = [] { return 2; };\n}\n",
        );
        let l0 = s.find("Identifier ?lambda0").expect("first lambda name:\n");
        let l1 = s.find("Identifier ?lambda1").expect("second lambda name");
        assert!(l0 < l1, "{s}");
        assert!(s.contains("ret: Auto"), "the default return type:\n{s}");
    }

    /// A `;` yields `Seq []` only as the first statement of its list; after
    /// any statement it is `hidden (skipMany semi)` noise (`Parser.hs:479,482`).
    #[test]
    fn only_a_leading_semicolon_is_an_empty_statement() {
        let leading = structure("inline void f()\n{\n    ;\n    return;\n}\n");
        assert!(leading.contains("Seq 0"), "{leading}");
        let trailing = structure("inline void f()\n{\n    return;\n    ;\n}\n");
        // body Seq holds exactly the `return` — `Seq 1` at the body level.
        assert!(trailing.contains("body: Seq 1"), "{trailing}");
    }

    /// `atomic s` is `Annotated (Seq [FlagAttr Atomic]) s` (`Parser.hs:559-561`).
    #[test]
    fn atomic_desugars_to_an_annotated_statement() {
        let s = structure("inline void f()\n{\n    atomic { barrier; }\n}\n");
        assert!(s.contains("Annotated"), "{s}");
        assert!(s.contains("FlagAttr atomic"), "{s}");
    }
}

/// The sorted lowering, graded against the reference's own trees.
///
/// Every `ORACLE_*` constant below is the **verbatim** output of
/// `kg-oracle --no-base --tree` (the reference frontend built by
/// `oracle/build.sh`) on the probe source above it, captured 2026-08-12. The
/// comparison is the pre-order constructor sequence — the same equality
/// `oracle/differential.py` checks — so these tests pin the sorter to the
/// reference, not to this port's reading of it.
#[cfg(test)]
mod sort_tests {
    use super::*;
    use crate::sort::NoImports;

    fn sorted(text: &str) -> crate::ModuleIr {
        let interner = Interner::new();
        let parse = kg_syntax::parse_file(text);
        assert!(parse.errors().is_empty(), "probe must parse clean: {:?}", parse.errors());
        let file = ast::SourceFile::cast(parse.syntax_node()).expect("root");
        lower_sorted(&file, FileId(0), &interner, &NoImports)
    }

    fn sorted_structure(text: &str) -> String {
        let interner = Interner::new();
        let parse = kg_syntax::parse_file(text);
        assert!(parse.errors().is_empty(), "probe must parse clean: {:?}", parse.errors());
        let file = ast::SourceFile::cast(parse.syntax_node()).expect("root");
        let ir = lower_sorted(&file, FileId(0), &interner, &NoImports);
        ir.structure_string(&interner)
    }

    /// Pre-order constructor names from a structural rendering — the `ours`
    /// half of `oracle/differential.py`.
    fn ours(structure: &str) -> Vec<String> {
        let mut out = Vec::new();
        for line in structure.lines() {
            let s = line.trim_start();
            if s.is_empty() || s.starts_with("seg:") || s.starts_with("import ") {
                continue;
            }
            let s = match s.split_once(": ") {
                Some((label, rest)) if label.chars().all(|c| c.is_ascii_lowercase() || c == '_') => rest,
                _ => s,
            };
            let word: String = s
                .chars()
                .take_while(|c| c.is_ascii_alphabetic())
                .collect();
            if word.chars().next().is_some_and(|c| c.is_ascii_uppercase()) {
                out.push(word);
            }
        }
        out
    }

    /// Pre-order constructor names from a `kg-oracle --tree` line: every
    /// upper-case word with the `F` suffix `show` prints, suffix stripped.
    /// The leading empty `SeqF []` — the redundant-import warning seed
    /// (`Parser.hs:234-241`) kg-hir deliberately omits — is dropped.
    fn theirs(tree: &str) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        let mut word = String::new();
        for c in tree.chars().chain([' ']) {
            if c.is_ascii_alphanumeric() {
                word.push(c);
                continue;
            }
            if word.len() > 1
                && word.ends_with('F')
                && word.chars().next().is_some_and(|c| c.is_ascii_uppercase())
                && word.chars().all(|c| c.is_ascii_alphabetic())
            {
                out.push(word[..word.len() - 1].to_string());
            }
            word.clear();
        }
        if out.len() >= 2 && out[0] == "Seq" && out[1] == "Seq" {
            out.remove(1);
        }
        out
    }

    #[track_caller]
    fn agree(source: &str, oracle: &str) {
        let structure = sorted_structure(source);
        let a = ours(&structure);
        let b = theirs(oracle);
        if a != b {
            let first = a
                .iter()
                .zip(b.iter())
                .position(|(x, y)| x != y)
                .unwrap_or(a.len().min(b.len()));
            panic!(
                "constructor sequences differ at #{first}: ours={:?} theirs={:?}\n\
                 ours   ...{:?}\n theirs ...{:?}\nfull structure:\n{structure}",
                a.get(first),
                b.get(first),
                &a[first.saturating_sub(3)..(first + 4).min(a.len())],
                &b[first.saturating_sub(3)..(first + 4).min(b.len())],
            );
        }
    }

    /// A local variable reference: `NamedValue → QualifiedIdentifier` over a
    /// `ScopedName` chain that materialises the declaration's whole qualifier
    /// (`makeQualified`, `Desugar.hs:188-201`).
    #[test]
    fn a_local_variable_reference_materialises_its_qualifier() {
        agree(
            "inline void main()\n{\n    uint8 x = 0;\n    uint8 y = x;\n}\n",
            r#"SeqF [SeqF [],FunctionF {declScopeName = [], funcKind = FreeFunction, funcAttr = SeqF [], funcModifier = Just FunctionModifierF inline, funcReturnType = VoidF, declName = IdentifierF "main", funcParams = SeqF [], body = SeqF [VariableF {declScopeName = ["main","$"], varAttr = SeqF [], varType = UnsignedF 8, declName = IdentifierF "x", varInit = Just IntLiteralF {literalType = Nothing, literalValue = 0}, declFlags = DeclareLocal},VariableF {declScopeName = ["main","$"], varAttr = SeqF [], varType = UnsignedF 8, declName = IdentifierF "y", varInit = Just NamedValueF QualifiedIdentifierF ScopedNameF ["main","$"] (Just ScopedNameF ["main","$"] (Just ScopedNameF ["main","$"] Nothing IdentifierF "main") IdentifierF "$") IdentifierF "x", declFlags = DeclareLocal}]}]"#,
        );
    }

    /// Forward and backward calls at the top level of a module-less file both
    /// stay unqualified: the declaration's scope is empty either way.
    #[test]
    fn top_level_calls_in_a_moduleless_file_stay_unqualified() {
        agree(
            "inline void f()\n{\n    g();\n}\n\ninline void g()\n{\n}\n\ninline void h()\n{\n    g();\n}\n",
            r#"SeqF [SeqF [],FunctionF {declScopeName = [], funcKind = FreeFunction, funcAttr = SeqF [], funcModifier = Just FunctionModifierF inline, funcReturnType = VoidF, declName = IdentifierF "f", funcParams = SeqF [], body = SeqF [FunctionCallF {callAttr = SeqF [], callFunction = FunctionSpecifierF Nothing QualifiedIdentifierF ScopedNameF ["f","$"] Nothing IdentifierF "g", callArgs = SeqF []}]},FunctionF {declScopeName = [], funcKind = FreeFunction, funcAttr = SeqF [], funcModifier = Just FunctionModifierF inline, funcReturnType = VoidF, declName = IdentifierF "g", funcParams = SeqF [], body = SeqF []},FunctionF {declScopeName = [], funcKind = FreeFunction, funcAttr = SeqF [], funcModifier = Just FunctionModifierF inline, funcReturnType = VoidF, declName = IdentifierF "h", funcParams = SeqF [], body = SeqF [FunctionCallF {callAttr = SeqF [], callFunction = FunctionSpecifierF Nothing QualifiedIdentifierF ScopedNameF ["h","$"] Nothing IdentifierF "g", callArgs = SeqF []}]}]"#,
        );
    }

    /// **Declaration order is observable** inside a module: a call written
    /// before `g`'s declaration resolves nothing and stays bare, one written
    /// after it gains the materialised `@m` prefix. This asymmetry is the
    /// whole reason the sorter's table is incremental.
    #[test]
    fn declaration_order_decides_qualification_inside_a_module() {
        let src = "module m\n{\n}\n\ninline void f()\n{\n    g();\n}\n\ninline void g()\n{\n}\n\ninline void h()\n{\n    g();\n}\n";
        agree(
            src,
            r#"SeqF [SeqF [],ModuleDeclF {declName = ModuleIdentifierF ["m"], exposed = [], body = SeqF [FunctionF {declScopeName = ["@m"], funcKind = FreeFunction, funcAttr = SeqF [], funcModifier = Just FunctionModifierF inline, funcReturnType = VoidF, declName = IdentifierF "f", funcParams = SeqF [], body = SeqF [FunctionCallF {callAttr = SeqF [], callFunction = FunctionSpecifierF Nothing QualifiedIdentifierF ScopedNameF ["@m","f","$"] Nothing IdentifierF "g", callArgs = SeqF []}]},FunctionF {declScopeName = ["@m"], funcKind = FreeFunction, funcAttr = SeqF [], funcModifier = Just FunctionModifierF inline, funcReturnType = VoidF, declName = IdentifierF "g", funcParams = SeqF [], body = SeqF []},FunctionF {declScopeName = ["@m"], funcKind = FreeFunction, funcAttr = SeqF [], funcModifier = Just FunctionModifierF inline, funcReturnType = VoidF, declName = IdentifierF "h", funcParams = SeqF [], body = SeqF [FunctionCallF {callAttr = SeqF [], callFunction = FunctionSpecifierF Nothing QualifiedIdentifierF ScopedNameF ["@m","h","$"] (Just ScopedNameF ["@m","h","$"] Nothing IdentifierF "@m") IdentifierF "g", callArgs = SeqF []}]}]}]"#,
        );
        // The constructor sequence alone cannot see *which* prefix appeared;
        // pin the asymmetry by name.
        let s = sorted_structure(src);
        let f = s.find("Identifier f").expect("f present");
        let h = s.find("Identifier h").expect("h present");
        assert!(!s[f..h].contains("Identifier @m"), "forward call must stay bare:\n{s}");
        assert!(s[h..].contains("Identifier @m"), "backward call gains the @m prefix:\n{s}");
    }

    /// Type positions: a resolved struct and a forward-referenced one both
    /// become `TypeIdentifier` — `typeSpecifier' True` allows unresolved
    /// names (`Parser.hs:724-725`).
    #[test]
    fn type_positions_sort_to_type_identifier_resolved_or_not() {
        agree(
            "struct Foo\n{\n    uint8 a;\n}\n\ninline void main()\n{\n    Foo x = {0};\n    Bar y = {0};\n}\n\nstruct Bar\n{\n    uint8 a;\n}\n",
            r#"SeqF [SeqF [],StructF {declScopeName = [], declName = IdentifierF "Foo", body = SeqF [VariableF {declScopeName = ["$quasi0"], varAttr = SeqF [], varType = UnsignedF 8, declName = IdentifierF "a", varInit = Nothing, declFlags = DeclareMember}]},FunctionF {declScopeName = [], funcKind = FreeFunction, funcAttr = SeqF [], funcModifier = Just FunctionModifierF inline, funcReturnType = VoidF, declName = IdentifierF "main", funcParams = SeqF [], body = SeqF [VariableF {declScopeName = ["main","$"], varAttr = SeqF [], varType = TypeIdentifierF ["main","$"] ScopedNameF ["main","$"] Nothing IdentifierF "Foo", declName = IdentifierF "x", varInit = Just InitializerListF SeqF [PositionalF 0 IntLiteralF {literalType = Nothing, literalValue = 0}], declFlags = DeclareLocal},VariableF {declScopeName = ["main","$"], varAttr = SeqF [], varType = TypeIdentifierF ["main","$"] ScopedNameF ["main","$"] Nothing IdentifierF "Bar", declName = IdentifierF "y", varInit = Just InitializerListF SeqF [PositionalF 0 IntLiteralF {literalType = Nothing, literalValue = 0}], declFlags = DeclareLocal}]},StructF {declScopeName = [], declName = IdentifierF "Bar", body = SeqF [VariableF {declScopeName = ["$quasi1"], varAttr = SeqF [], varType = UnsignedF 8, declName = IdentifierF "a", varInit = Nothing, declFlags = DeclareMember}]}]"#,
        );
    }

    /// Templates: parameter references become bare `TypeParam`; `Vec<uint8,4>`
    /// in type position is `TypeIdentifier(TemplateInstance …)`; a template
    /// function call keeps its written arguments, and one *without* any still
    /// becomes `TemplateInstance … (Seq [])` (`Parser.hs:694`).
    #[test]
    fn template_declarations_instances_and_calls() {
        agree(
            "template <typename T, uint32 N>\nstruct Vec\n{\n    T[N] data;\n}\n\ntemplate <typename T>\ninline T id(T v)\n{\n    return v;\n}\n\ninline void main()\n{\n    Vec<uint8, 4> v = {{0, 1, 2, 3}};\n    uint8 a = id<uint8>(3);\n    uint8 b = id(a);\n}\n",
            r#"SeqF [SeqF [],TemplateF {templateParams = [TemplateParamF [] TypenameF "T" 0 Nothing,TemplateParamF [] UnsignedF 32 "N" 1 Nothing], templateDecl = StructF {declScopeName = [], declName = IdentifierF "Vec", body = SeqF [VariableF {declScopeName = ["$quasi0"], varAttr = SeqF [], varType = ArrayF SeqF [] TypeParamF [] TypenameF "T" 0 SeqF [TypeParamF [] UnsignedF 32 "N" 1], declName = IdentifierF "data", varInit = Nothing, declFlags = DeclareMember}]}},TemplateF {templateParams = [TemplateParamF [] TypenameF "T" 0 Nothing], templateDecl = FunctionF {declScopeName = [], funcKind = FreeFunction, funcAttr = SeqF [], funcModifier = Just FunctionModifierF inline, funcReturnType = TypeParamF [] TypenameF "T" 0, declName = IdentifierF "id", funcParams = SeqF [FuncParamF {declScopeName = ["id"], paramAttr = SeqF [], paramType = TypeParamF [] TypenameF "T" 0, declName = IdentifierF "v"}], body = SeqF [ReturnF (Just NamedValueF QualifiedIdentifierF ScopedNameF ["id","$"] (Just ScopedNameF ["id","$"] Nothing IdentifierF "id") IdentifierF "v")]}},FunctionF {declScopeName = [], funcKind = FreeFunction, funcAttr = SeqF [], funcModifier = Just FunctionModifierF inline, funcReturnType = VoidF, declName = IdentifierF "main", funcParams = SeqF [], body = SeqF [VariableF {declScopeName = ["main","$"], varAttr = SeqF [], varType = TypeIdentifierF ["main","$"] TemplateInstanceF ScopedNameF ["main","$"] Nothing IdentifierF "Vec" SeqF [UnsignedF 8,IntLiteralF {literalType = Nothing, literalValue = 4}], declName = IdentifierF "v", varInit = Just InitializerListF SeqF [PositionalF 0 InitializerListF SeqF [PositionalF 0 IntLiteralF {literalType = Nothing, literalValue = 0},PositionalF 1 IntLiteralF {literalType = Nothing, literalValue = 1},PositionalF 2 IntLiteralF {literalType = Nothing, literalValue = 2},PositionalF 3 IntLiteralF {literalType = Nothing, literalValue = 3}]], declFlags = DeclareLocal},VariableF {declScopeName = ["main","$"], varAttr = SeqF [], varType = UnsignedF 8, declName = IdentifierF "a", varInit = Just FunctionCallF {callAttr = SeqF [], callFunction = FunctionSpecifierF Nothing QualifiedIdentifierF TemplateInstanceF ScopedNameF ["main","$"] Nothing IdentifierF "id" SeqF [UnsignedF 8], callArgs = SeqF [IntLiteralF {literalType = Nothing, literalValue = 3}]}, declFlags = DeclareLocal},VariableF {declScopeName = ["main","$"], varAttr = SeqF [], varType = UnsignedF 8, declName = IdentifierF "b", varInit = Just FunctionCallF {callAttr = SeqF [], callFunction = FunctionSpecifierF Nothing QualifiedIdentifierF TemplateInstanceF ScopedNameF ["main","$"] Nothing IdentifierF "id" SeqF [], callArgs = SeqF [NamedValueF QualifiedIdentifierF ScopedNameF ["main","$"] (Just ScopedNameF ["main","$"] (Just ScopedNameF ["main","$"] Nothing IdentifierF "main") IdentifierF "$") IdentifierF "a"]}, declFlags = DeclareLocal}]}]"#,
        );
    }

    /// Class members resolve through the lookahead seeds regardless of kind;
    /// a method callee's written prefix is the `$unresolved` placeholder even
    /// when the receiver's type is fully known (`Parser.hs:730-734,764`).
    #[test]
    fn class_members_and_method_callees() {
        let src = "class Counter\n{\n    uint32 count;\n\n    inline void bump()\n    {\n        count += 1;\n    }\n\n    inline uint32 get()\n    {\n        bump();\n        return count;\n    }\n}\n\ninline void main()\n{\n    Counter c;\n    c.bump();\n    uint32 v = c.get();\n}\n";
        agree(
            src,
            r#"SeqF [SeqF [],ClassF {declScopeName = [], declName = IdentifierF "Counter", body = SeqF [VariableF {declScopeName = ["Counter"], varAttr = SeqF [], varType = UnsignedF 32, declName = IdentifierF "count", varInit = Nothing, declFlags = DeclareClass},FunctionF {declScopeName = ["Counter"], funcKind = MemberFunction, funcAttr = SeqF [], funcModifier = Just FunctionModifierF inline, funcReturnType = VoidF, declName = IdentifierF "bump", funcParams = SeqF [], body = SeqF [AssignF NamedValueF QualifiedIdentifierF ScopedNameF ["Counter","bump","$"] (Just ScopedNameF ["Counter","bump","$"] Nothing IdentifierF "Counter") IdentifierF "count" BinaryF Add NamedValueF QualifiedIdentifierF ScopedNameF ["Counter","bump","$"] (Just ScopedNameF ["Counter","bump","$"] Nothing IdentifierF "Counter") IdentifierF "count" IntLiteralF {literalType = Nothing, literalValue = 1}]},FunctionF {declScopeName = ["Counter"], funcKind = MemberFunction, funcAttr = SeqF [], funcModifier = Just FunctionModifierF inline, funcReturnType = UnsignedF 32, declName = IdentifierF "get", funcParams = SeqF [], body = SeqF [FunctionCallF {callAttr = SeqF [], callFunction = FunctionSpecifierF Nothing QualifiedIdentifierF ScopedNameF ["Counter","get","$"] (Just ScopedNameF ["Counter","get","$"] Nothing IdentifierF "Counter") IdentifierF "bump", callArgs = SeqF []},ReturnF (Just NamedValueF QualifiedIdentifierF ScopedNameF ["Counter","get","$"] (Just ScopedNameF ["Counter","get","$"] Nothing IdentifierF "Counter") IdentifierF "count")]}]},FunctionF {declScopeName = [], funcKind = FreeFunction, funcAttr = SeqF [], funcModifier = Just FunctionModifierF inline, funcReturnType = VoidF, declName = IdentifierF "main", funcParams = SeqF [], body = SeqF [VariableF {declScopeName = ["main","$"], varAttr = SeqF [], varType = TypeIdentifierF ["main","$"] ScopedNameF ["main","$"] Nothing IdentifierF "Counter", declName = IdentifierF "c", varInit = Nothing, declFlags = DeclareLocal},FunctionCallF {callAttr = SeqF [], callFunction = FunctionSpecifierF (Just NamedValueF QualifiedIdentifierF ScopedNameF ["main","$"] (Just ScopedNameF ["main","$"] (Just ScopedNameF ["main","$"] Nothing IdentifierF "main") IdentifierF "$") IdentifierF "c") QualifiedIdentifierF ScopedNameF ["main","$"] (Just ScopedNameF [] Nothing IdentifierF "$unresolved") IdentifierF "bump", callArgs = SeqF []},VariableF {declScopeName = ["main","$"], varAttr = SeqF [], varType = UnsignedF 32, declName = IdentifierF "v", varInit = Just FunctionCallF {callAttr = SeqF [], callFunction = FunctionSpecifierF (Just NamedValueF QualifiedIdentifierF ScopedNameF ["main","$"] (Just ScopedNameF ["main","$"] (Just ScopedNameF ["main","$"] Nothing IdentifierF "main") IdentifierF "$") IdentifierF "c") QualifiedIdentifierF ScopedNameF ["main","$"] (Just ScopedNameF [] Nothing IdentifierF "$unresolved") IdentifierF "get", callArgs = SeqF []}, declFlags = DeclareLocal}]}]"#,
        );
        let s = sorted_structure(src);
        assert!(s.contains("Identifier $unresolved"), "the placeholder prefix, by name:\n{s}");
    }

    /// Enum constants see earlier constants; `Color::Red` keeps its written
    /// prefix; `Shade::Blue` resolves *through* the alias target but keeps
    /// the alias's spelling (`Parser.hs:667-668`).
    #[test]
    fn enum_constants_and_alias_prefixes() {
        agree(
            "enum Color : uint2\n{\n    Red,\n    Green = 1,\n    Blue = Green\n}\n\nusing Shade = Color;\n\ninline void main()\n{\n    Color c = Color::Red;\n    Shade s = Shade::Blue;\n    uint2 v = cast<uint2>(c);\n}\n",
            r#"SeqF [SeqF [],EnumF {declScopeName = [], declName = IdentifierF "Color", enumType = UnsignedF 2, body = SeqF [EnumConstantF {declScopeName = ["Color"], declName = IdentifierF "Red", constValue = Nothing},EnumConstantF {declScopeName = ["Color"], declName = IdentifierF "Green", constValue = Just IntLiteralF {literalType = Nothing, literalValue = 1}},EnumConstantF {declScopeName = ["Color"], declName = IdentifierF "Blue", constValue = Just NamedValueF QualifiedIdentifierF ScopedNameF ["Color"] (Just ScopedNameF ["Color"] Nothing IdentifierF "Color") IdentifierF "Green"}]},AliasF {declScopeName = [], body = TypeIdentifierF [] ScopedNameF [] Nothing IdentifierF "Color", declName = IdentifierF "Shade"},FunctionF {declScopeName = [], funcKind = FreeFunction, funcAttr = SeqF [], funcModifier = Just FunctionModifierF inline, funcReturnType = VoidF, declName = IdentifierF "main", funcParams = SeqF [], body = SeqF [VariableF {declScopeName = ["main","$"], varAttr = SeqF [], varType = TypeIdentifierF ["main","$"] ScopedNameF ["main","$"] Nothing IdentifierF "Color", declName = IdentifierF "c", varInit = Just NamedValueF QualifiedIdentifierF ScopedNameF ["main","$"] (Just ScopedNameF ["main","$"] Nothing IdentifierF "Color") IdentifierF "Red", declFlags = DeclareLocal},VariableF {declScopeName = ["main","$"], varAttr = SeqF [], varType = TypeIdentifierF ["main","$"] ScopedNameF ["main","$"] Nothing IdentifierF "Shade", declName = IdentifierF "s", varInit = Just NamedValueF QualifiedIdentifierF ScopedNameF ["main","$"] (Just ScopedNameF ["main","$"] Nothing IdentifierF "Shade") IdentifierF "Blue", declFlags = DeclareLocal},VariableF {declScopeName = ["main","$"], varAttr = SeqF [], varType = UnsignedF 2, declName = IdentifierF "v", varInit = Just CastF UnsignedF 2 NamedValueF QualifiedIdentifierF ScopedNameF ["main","$"] (Just ScopedNameF ["main","$"] (Just ScopedNameF ["main","$"] Nothing IdentifierF "main") IdentifierF "$") IdentifierF "c", declFlags = DeclareLocal}]}]"#,
        );
    }

    /// Template arguments: a struct sorts as `TypeIdentifier` (the `S1<Foo>`
    /// shape divergence #22 refuses to *error* on), a resolved constant as
    /// `NamedValue`, and a forward-referenced one as `NamedValue` too —
    /// `expressionTemplate` does not allow unresolved *types*
    /// (`typ' False`), so the name falls through to `value`.
    #[test]
    fn template_arguments_sort_by_symbol_and_declaration_order() {
        agree(
            "struct Foo\n{\n    uint8 a;\n}\n\ntemplate <typename T, uint32 N>\nstruct Holder\n{\n    T[N] items;\n}\n\nconst uint32 SIZE = 6;\n\ninline void main()\n{\n    Holder<Foo, SIZE> h;\n    Holder<Foo, Later> g;\n}\n\nconst uint32 Later = 2;\n",
            r#"SeqF [SeqF [],StructF {declScopeName = [], declName = IdentifierF "Foo", body = SeqF [VariableF {declScopeName = ["$quasi0"], varAttr = SeqF [], varType = UnsignedF 8, declName = IdentifierF "a", varInit = Nothing, declFlags = DeclareMember}]},TemplateF {templateParams = [TemplateParamF [] TypenameF "T" 0 Nothing,TemplateParamF [] UnsignedF 32 "N" 1 Nothing], templateDecl = StructF {declScopeName = [], declName = IdentifierF "Holder", body = SeqF [VariableF {declScopeName = ["$quasi1"], varAttr = SeqF [], varType = ArrayF SeqF [] TypeParamF [] TypenameF "T" 0 SeqF [TypeParamF [] UnsignedF 32 "N" 1], declName = IdentifierF "items", varInit = Nothing, declFlags = DeclareMember}]}},VariableF {declScopeName = [], varAttr = SeqF [], varType = ConstF UnsignedF 32, declName = IdentifierF "SIZE", varInit = Just IntLiteralF {literalType = Nothing, literalValue = 6}, declFlags = DeclareGlobal},FunctionF {declScopeName = [], funcKind = FreeFunction, funcAttr = SeqF [], funcModifier = Just FunctionModifierF inline, funcReturnType = VoidF, declName = IdentifierF "main", funcParams = SeqF [], body = SeqF [VariableF {declScopeName = ["main","$"], varAttr = SeqF [], varType = TypeIdentifierF ["main","$"] TemplateInstanceF ScopedNameF ["main","$"] Nothing IdentifierF "Holder" SeqF [TypeIdentifierF ["main","$"] ScopedNameF ["main","$"] Nothing IdentifierF "Foo",NamedValueF QualifiedIdentifierF ScopedNameF ["main","$"] Nothing IdentifierF "SIZE"], declName = IdentifierF "h", varInit = Nothing, declFlags = DeclareLocal},VariableF {declScopeName = ["main","$"], varAttr = SeqF [], varType = TypeIdentifierF ["main","$"] TemplateInstanceF ScopedNameF ["main","$"] Nothing IdentifierF "Holder" SeqF [TypeIdentifierF ["main","$"] ScopedNameF ["main","$"] Nothing IdentifierF "Foo",NamedValueF QualifiedIdentifierF ScopedNameF ["main","$"] Nothing IdentifierF "Later"], declName = IdentifierF "g", varInit = Nothing, declFlags = DeclareLocal}]},VariableF {declScopeName = [], varAttr = SeqF [], varType = ConstF UnsignedF 32, declName = IdentifierF "Later", varInit = Just IntLiteralF {literalType = Nothing, literalValue = 2}, declFlags = DeclareGlobal}]"#,
        );
    }

    /// **The outermost sorted node is the one that claims the slot.**
    ///
    /// `field_callee` hands a method callee's whole path the `NAME_REF` as its
    /// donor, so the leaf `Identifier` built from that node and the
    /// `QualifiedIdentifier` wrapped round it land on one `(range, kind)` —
    /// the `TypeKey` a position query is answered from. The outer node keeps
    /// `Own` because it is what such a query should return; the leaf donates.
    /// The *shapes* are the reference's own (`copyNote (ScopedNameF …) ident`,
    /// `Parser.hs:655`; `copyNote (QualifiedIdentifierF s) s`, `:644`, which
    /// give three nested nodes one identical note) and are not touched — see
    /// divergence #24, closed by this.
    #[test]
    fn a_method_callees_name_ref_is_claimed_by_the_outermost_node_alone() {
        let ir = sorted(
            "class Counter\n{\n    inline void bump()\n    {\n    }\n}\n\n\
             inline void main()\n{\n    Counter c;\n    c.bump();\n}\n",
        );
        let callee = ir
            .arena
            .iter()
            .find(|(_, n)| {
                matches!(n.exp, Exp::QualifiedIdentifier(_)) && n.kind == SyntaxKind::NAME_REF
            })
            .expect("the method callee is a QualifiedIdentifier at the NAME_REF");
        let (outer, key) = (callee.0, (callee.1.src.head, callee.1.kind));
        assert_eq!(callee.1.src.written, Written::Own, "the outermost node claims it");

        // The leaf `Identifier` under it wears the same range and kind — the
        // reference's shared note — and must not claim it.
        let mut claimants = Vec::new();
        let mut leaves = 0;
        for (id, node) in ir.arena.iter() {
            if (node.src.head, node.kind) != key {
                continue;
            }
            if node.src.written == Written::Own {
                claimants.push(id);
            }
            if matches!(node.exp, Exp::Identifier(_)) {
                leaves += 1;
                assert_eq!(
                    node.src.written,
                    Written::Donated,
                    "the leaf Identifier inside a sorted path donates"
                );
            }
        }
        // Two `Identifier`s wear that node: the callee's own leaf, and the
        // `$unresolved` placeholder `methodSpecifier` starts pre-qualified
        // with (`Parser.hs:730-734,764`), which is synthesized and so was
        // never a claimant.
        assert_eq!(leaves, 2, "the leaf and the `$unresolved` placeholder");
        assert_eq!(claimants, vec![outer], "exactly one Own node on that TypeKey");
    }

    /// The invariant the two demotions above exist for, at unit scale — the
    /// same thing `xtask ir`'s provenance clause three enforces corpus-wide:
    /// **no `(range, kind)` is claimed `Own` twice**, so a `TypeKey` never has
    /// two answers. The probe holds every shape that used to break it: a
    /// method callee, a compound assignment, an `++`, an indexed lvalue and a
    /// qualified name.
    #[test]
    fn no_two_sorted_nodes_claim_one_type_key() {
        let ir = sorted(
            "class Counter\n{\n    uint32 count;\n    uint32[4] slots;\n\n\
             inline void bump()\n    {\n        count += 1;\n        count++;\n        \
             slots[0] += count;\n    }\n}\n\n\
             inline void main()\n{\n    Counter c;\n    c.bump();\n    uint32 v = c.count;\n}\n",
        );
        let mut seen: std::collections::BTreeMap<(u32, u32, SyntaxKind), usize> =
            std::collections::BTreeMap::new();
        let mut stack = vec![ir.root];
        let mut children = Vec::new();
        while let Some(id) = stack.pop() {
            let node = &ir.arena[id];
            if node.src.written == Written::Own {
                let r = node.src.head.range;
                *seen.entry((r.start().into(), r.end().into(), node.kind)).or_default() += 1;
            }
            node.exp.children(&mut children);
            stack.append(&mut children);
        }
        let doubled: Vec<_> = seen.iter().filter(|&(_, &n)| n > 1).collect();
        assert!(doubled.is_empty(), "one TypeKey, two Own claimants: {doubled:?}");
    }

    /// The unsorted counter: a lowering without a symbol context sorts
    /// nothing and counts nothing; a sorted one reports what it declined.
    #[test]
    fn sort_stats_report_the_mode() {
        let interner = Interner::new();
        let parse = kg_syntax::parse_file("inline void f()\n{\n    g();\n}\n");
        let file = ast::SourceFile::cast(parse.syntax_node()).expect("root");
        let plain = lower(&file, FileId(0), &interner);
        assert!(!plain.sort.attempted);
        let s = plain.structure_string(&interner);
        assert!(s.contains("Path"), "no context, no sorting:\n{s}");

        let ir = sorted("inline void f()\n{\n    g();\n}\n");
        assert!(ir.sort.attempted);
        assert!(ir.sort.unsorted.is_empty());
    }
}
