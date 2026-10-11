//! What the reference's parser built of a module, read off the sorted
//! lowering — the facts the parser's own checks ask of a name, which until
//! sprint W3.7 the CST engine's type table answered.
//!
//! The reference sorts every written name as it parses, against the
//! parse-time symbol table (`Parser.hs:652-801`): a name the table holds as
//! a type is parsed as one, and `expression` then rejects it where a value
//! goes (`Parser.hs:823-828`); a method written with `<…>` opens an argument
//! list only where the receiver's type name holds a template of that name
//! (`methodSpecifier`, `:730-762`), and is "undefined template" otherwise
//! (`methodInstanceSpecifier`, `:766-770`). [`kg_hir::lower_sorted`] is that
//! sort, against [`kg_hir::sort::SymTab`] — the parser's table restated —
//! and graded against the reference's own tree by `xtask ir`'s oracle
//! differential. So these are the parser's answers, where the CST table's
//! were an inference engine's guess at them: the type it inferred for a
//! name, and for a receiver the class it inferred, not the type name the
//! parser resolves.
//!
//! The sort's reading of a method is not taken alone: under a receiver's
//! type the parser never resolved (`fj.fork<0>(…)` over a `fork_join<…>`
//! no import reaches, `test/library/control/async.k` under `mini-base`) the
//! reference's parser has already failed at the type, so the
//! member-template check asks the loop's types to confirm it
//! (`passes::undefined_symbols`).

use kg_base::{FxHashSet, TextRange, Written};
use kg_hir::ModuleIr;
use kg_hir::sort::UnsortedReason;

/// One module's parse, as its parser checks ask it.
#[derive(Debug, Default)]
pub struct Parsed {
    /// The range of every node the parser built as a type (`isType`,
    /// `Parser/Syntax.hs:676-699`) among those the module's own text wrote
    /// and its tree holds.
    types: FxHashSet<TextRange>,
    /// The name of every method written with a template argument list and
    /// no `template` keyword that the sort found no template for under the
    /// receiver's type name — the parser's "undefined template"
    /// (`Parser.hs:766-770`), as the sort reads it.
    not_templates: FxHashSet<TextRange>,
    /// Every path written as a type that the table holds as a value —
    /// `typeSpecifier'`'s `unexpectedSymbol s "type specifier"`
    /// (`Parser.hs:723`), as the sort reads it.
    values_as_types: FxHashSet<TextRange>,
}

impl Parsed {
    /// The facts of `ir`, a module's sorted lowering.
    pub fn of(ir: &ModuleIr) -> Parsed {
        let mut types = FxHashSet::default();
        // The tree, not the arena: a sort that failed leaves the nodes it
        // had allocated in the arena, unreachable, and a name it took for a
        // template parameter's use there is no type the parser built.
        for id in crate::sem::reachable(ir) {
            let node = &ir.arena[id];
            if node.src.written == Written::Own
                && node.src.head.file == ir.file
                && kg_hir::sort::is_type(&ir.arena, id)
                && !unsorted_kind(&ir.arena, id)
            {
                types.insert(node.src.head.range);
            }
        }
        let unsorted = |reason: UnsortedReason| -> FxHashSet<TextRange> {
            ir.sort.unsorted.iter().filter(|(at, why)| at.file == ir.file && *why == reason).map(|(at, _)| at.range).collect()
        };
        Parsed {
            types,
            not_templates: unsorted(UnsortedReason::ArgsOnNonTemplate),
            values_as_types: unsorted(UnsortedReason::ValueInTypePosition),
        }
    }

    /// Whether the parser built the expression at `range` as a type.
    /// Answered for a name in a value position: `isType` of what the parser
    /// built there (`Parser.hs:823-828`).
    pub fn is_type(&self, range: TextRange) -> bool {
        self.types.contains(&range)
    }

    /// Whether the sort read the method name at `range`, written with
    /// `<…>`, as opening no template argument list.
    pub fn opens_no_template(&self, name: TextRange) -> bool {
        self.not_templates.contains(&name)
    }

    /// Whether the path at `range`, written as a type, names what the
    /// parse-time table holds as a value there.
    pub fn is_value_as_type(&self, range: TextRange) -> bool {
        self.values_as_types.contains(&range)
    }
}

/// Whether `id` is a use of a template parameter whose kind the sort left
/// an unsorted path — `index_t<Banks> Bix` where no `index_t` template is in
/// the parse-time table, so the reference's parser read no argument list
/// there and failed (`Parser.hs:690-695`; divergence #7). `isType` of the
/// use is `not (isType kind)` (`Parser/Syntax.hs:696-697`), which of a kind
/// the parser never built says nothing; the use is left a value, toward
/// silence (`test/library/data/memory/bank/schedule.k` under `mini-base`,
/// whose `index_t` the base does not reach).
fn unsorted_kind(arena: &kg_hir::ExpArena, id: kg_hir::ExpId) -> bool {
    use kg_hir::Exp;
    match &arena[id].exp {
        Exp::TypeParam { kind, .. } | Exp::TemplateParam { kind, .. } => match &arena[*kind].exp {
            Exp::Path(_) => true,
            Exp::Const(inner) => matches!(arena[*inner].exp, Exp::Path(_)),
            _ => false,
        },
        Exp::TemplateInstance { base, .. } => unsorted_kind(arena, *base),
        _ => false,
    }
}
