//! Owner-topic: **the value domain of the constexpr evaluator**.
//!
//! Spec: `docs/spec/types.md` §3 (literal typing), §3.3 (constant folding and
//! literal re-typing); `docs/spec/resolve.md` §9.3 (what must fully fold before
//! the device-config consumer sees it).
//!
//! The reference has no `ConstValue` type: folding rewrites AST nodes in place,
//! so a "value" is whichever of `IntLiteralF`, `BoolLiteralF`, `FloatLiteralF`,
//! `StringLiteralF`, `EnumValueF` or `InitializerListF` the fold produced
//! (`Constexpr.hs:171-250`, `Parser/Syntax.hs:347-357`). [`ConstValue`] is the
//! reification of exactly that set, so a fold result can be written back to a
//! literal node without loss.

use kg_base::{SmolStr, SymbolPath};

use crate::ConstInt;

/// `data LiteralTypeSignedness = SignedLiteral | UnsignedLiteral`
/// (`Parser/Syntax.hs:211-214`), spelled `i` and `u` in a literal suffix
/// (`Parser/Syntax.hs:223-225`).
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum LiteralSign {
    Signed,
    Unsigned,
}

impl LiteralSign {
    /// The suffix letter: `show SignedLiteral = "i"` (`Parser/Syntax.hs:223-225`).
    pub fn suffix(self) -> &'static str {
        match self {
            LiteralSign::Signed => "i",
            LiteralSign::Unsigned => "u",
        }
    }
}

/// `data LiteralType = LiteralType { literalSignedness, literalWidth }`
/// (`Parser/Syntax.hs:216-221`) — the `i32`/`u8` suffix on an integer literal.
///
/// Absent (`None` on [`ConstValue::Int`]) means an *untyped* literal, whose type
/// is re-derived from its value on every round (`Type.hs:485-491`). That
/// distinction is load-bearing: a fold of two untyped literals stays untyped
/// (`Constexpr.hs:200-202`), which is why constant folding silences narrowing
/// conversion warnings (`docs/spec/types.md` §15 trap 19).
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct LiteralType {
    pub sign: LiteralSign,
    pub width: u32,
}

impl LiteralType {
    pub fn signed(width: u32) -> LiteralType {
        LiteralType { sign: LiteralSign::Signed, width }
    }

    pub fn unsigned(width: u32) -> LiteralType {
        LiteralType { sign: LiteralSign::Unsigned, width }
    }

    pub fn is_signed(self) -> bool {
        self.sign == LiteralSign::Signed
    }

    /// The `Nothing` branch of `inferIntLiteralType` (`Type.hs:485-491`),
    /// i.e. the *minimal* type an unsuffixed literal re-derives every round:
    ///
    /// ```text
    /// inferIntLiteralType Nothing x
    ///     | x == 0    = TUnsigned 1
    ///     | x < 0     = TSigned  (1 + clog2 (abs x))
    ///     | otherwise = TUnsigned (clog2 (x + 1))
    /// ```
    ///
    /// Worked values are pinned by `test/syntax/constexpr.k:436-479`
    /// (`bitsizeof 0 == 1`, `bitsizeof 3 == 2`, `bitsizeof 255 == 8`,
    /// `bitsizeof 256 == 9`).
    ///
    /// The one copy of the rule (`single-source-of-truth.md` D1.6): the value
    /// domain needs it — `evalUnary Invert` truncates into exactly this width
    /// (`Constexpr.hs:263`) and `evalBinaryInt` compares against exactly this
    /// `sizeOf` (`Constexpr.hs:231-235`) — and `kg_ty::width::minimal_int_type`
    /// is its image in `Ty`, since `kg-ty` depends on `kg-const`, never the
    /// reverse.
    pub fn minimal(value: &ConstInt) -> LiteralType {
        if value.is_zero() {
            LiteralType::unsigned(1)
        } else if value.is_negative() {
            // `1 + clog2 (abs x)`; `abs x > 0` here, so `clog2` is defined.
            LiteralType::signed(1 + value.abs().clog2())
        } else {
            // `clog2 (x + 1)`; `x + 1 > 1 > 0`.
            LiteralType::unsigned(value.add(&ConstInt::ONE).clog2())
        }
    }

    /// `tSigned` (`Type.hs:550-551`): `intW` is already signed, `uintW` promotes
    /// to `int(W+1)`. Used by `evalUnary Invert` on an *untyped* literal
    /// (`Constexpr.hs:263`).
    pub fn to_signed(self) -> LiteralType {
        match self.sign {
            LiteralSign::Signed => self,
            LiteralSign::Unsigned => LiteralType::signed(self.width + 1),
        }
    }

    /// `truncateInt`/`truncateUInt` by this stamp's sign (`Internal.hs:34-41`):
    /// what `validateArg`'s re-stamp leaves of an argument
    /// (`IntLiteralF (Just t) (truncate… width i)`, `TypeCheck.hs:281-283`),
    /// and the `outOfRange` test of a suffixed literal (`Frontend.hs:214-216`).
    pub fn truncate(self, value: &ConstInt) -> ConstInt {
        match self.sign {
            LiteralSign::Signed => value.truncate_int(self.width),
            LiteralSign::Unsigned => value.truncate_uint(self.width),
        }
    }

    /// Parses an `i<W>` / `u<W>` literal suffix (`Parser.hs:904-908`).
    /// `None` for the empty string (an unsuffixed literal) *and* for anything
    /// that is not a suffix — callers distinguish with [`str::is_empty`].
    pub fn parse_suffix(text: &str) -> Option<LiteralType> {
        let sign = match text.as_bytes().first()? {
            b'i' => LiteralSign::Signed,
            b'u' => LiteralSign::Unsigned,
            _ => return None,
        };
        let width = &text[1..];
        if width.is_empty() || !width.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        // `L.decimal` is unbounded in Haskell; a width past `u32` is rejected
        // downstream by `validateTypedLiterals` (`Frontend.hs:210-217`) long
        // before it can be a real type, so saturating is safe here.
        Some(LiteralType { sign, width: width.parse().unwrap_or(u32::MAX) })
    }
}

impl std::fmt::Display for LiteralType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}{}", self.sign.suffix(), self.width)
    }
}

/// A 32-bit float held as its IEEE-754 bit pattern.
///
/// The front-end already treats float constants as bit patterns —
/// `castWord32ToFloat` / `castFloatToWord32` implement `cast<float32>` and
/// `cast<uintN>` by reinterpretation (`Constexpr.hs:329-339`) — and storing bits
/// is what makes [`ConstValue`] `Eq`, `Ord` and `Hash`, which `f32` is not.
///
/// **Ordering is over the bit pattern, not the numeric value.** It exists to make
/// this type usable as a map key; never use it to compare magnitudes.
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct F32Bits(pub u32);

impl F32Bits {
    pub fn from_f32(v: f32) -> F32Bits {
        F32Bits(v.to_bits())
    }

    pub fn to_f32(self) -> f32 {
        f32::from_bits(self.0)
    }

    /// GHC's `show :: Float -> String` — what the reference embeds in a
    /// mangled name (`Mangle.hs:41`, `"@" <> show x`) and in an instance's
    /// display (`PrettyPrint.hs:105`, `pretty x`, which is `viaShow`):
    /// `showSignedFloat showFloat` over `formatRealFloat FFGeneric Nothing`
    /// (`GHC.Float`) — fixed notation for a decimal exponent in `0..=7`,
    /// `d.ddde<n>` otherwise, always a fractional digit, `NaN`,
    /// `Infinity`, `-0.0`. The digits are [`float_to_digits`]'s, not Rust's
    /// shortest round trip, whose boundary rule differs.
    pub fn haskell_show(self) -> String {
        let x = self.to_f32();
        if x.is_nan() {
            return "NaN".to_string();
        }
        let sign = if x.is_sign_negative() { "-" } else { "" };
        if x.is_infinite() {
            return format!("{sign}Infinity");
        }
        let (digits, e) = float_to_digits(self.0 & 0x7fff_ffff);
        let ds: String = digits.iter().map(|d| char::from(b'0' + *d)).collect();
        let body = if !(0..=7).contains(&e) {
            // `FFExponent`, `decs = Nothing`.
            match ds.as_str() {
                "0" => "0.0e0".to_string(),
                _ if ds.len() == 1 => format!("{ds}.0e{}", e - 1),
                _ => format!("{}.{}e{}", &ds[..1], &ds[1..], e - 1),
            }
        } else if e == 0 {
            // `FFFixed`, `e <= 0`: `"0." ++ replicate (-e) '0' ++ ds`.
            format!("0.{ds}")
        } else {
            // `f e "" ds`: the first `e` digits, padded with zeros, then
            // the rest, each side `mk0` (an empty side is `0`).
            let e = e as usize;
            let (int, frac) = if ds.len() >= e { (ds[..e].to_string(), &ds[e..]) } else { (format!("{ds:0<e$}"), "") };
            format!("{int}.{}", if frac.is_empty() { "0" } else { frac })
        };
        format!("{sign}{body}")
    }
}

/// GHC's `floatToDigits 10` (`GHC.Float`) for a non-negative finite `Float`,
/// by its bits: Burger and Dybvig's free-format digit generation, with
/// GHC's strict boundary tests — a digit string that lands exactly on the
/// rounding interval's edge is never taken, whatever the mantissa's
/// parity, and a tie rounds the last digit up. Returns the digits and the
/// decimal exponent `k` (the value is `0.d1d2… × 10^k`).
fn float_to_digits(bits: u32) -> (Vec<u8>, i32) {
    use num_bigint::BigUint;
    if bits == 0 {
        return (vec![0], 0);
    }
    // `decodeFloat` with GHC's denormal adjustment: `(f, e)` with the
    // exponent never below `minExp = -149`.
    let (biased, frac) = ((bits >> 23) as i32, bits & 0x7f_ffff);
    let (f, e) = if biased == 0 { (frac, -149) } else { (frac | 0x80_0000, biased - 150) };
    let p = 24;
    let min_exp = -149;
    let two = || BigUint::from(2u32);
    let big = |n: u32| BigUint::from(n);
    let pow2 = |n: i32| BigUint::from(1u32) << n as usize;
    let f_big = big(f);
    let (r, s, m_up, m_dn) = if e >= 0 {
        let be = pow2(e);
        if f == 1 << (p - 1) {
            (&f_big * &be * two() * two(), two() * two(), &be * two(), be)
        } else {
            (&f_big * &be * two(), two(), be.clone(), be)
        }
    } else if e > min_exp && f == 1 << (p - 1) {
        (&f_big * two() * two(), pow2(-e + 1) * two(), two(), big(1))
    } else {
        (&f_big * two(), pow2(-e) * two(), big(1), big(1))
    };
    // `k`: the least `n` with `r + mUp <= 10^n * s` (`fixup` from an
    // estimate that never overshoots).
    let ten = || BigUint::from(10u32);
    let fits = |n: i32| -> bool {
        if n >= 0 { &r + &m_up <= ten().pow(n as u32) * &s } else { ten().pow((-n) as u32) * (&r + &m_up) <= s }
    };
    let mut k = -50;
    while !fits(k) {
        k += 1;
    }
    let (mut rn, s_n, mut m_up_n, mut m_dn_n) = if k >= 0 {
        (r.clone(), &s * ten().pow(k as u32), m_up.clone(), m_dn.clone())
    } else {
        let bk = ten().pow((-k) as u32);
        (&r * &bk, s.clone(), &m_up * &bk, &m_dn * &bk)
    };
    let mut digits = Vec::new();
    loop {
        let scaled = &rn * ten();
        let dn = &scaled / &s_n;
        let rn2 = scaled % &s_n;
        m_up_n *= ten();
        m_dn_n *= ten();
        let dn: u8 = u8::try_from(&dn).expect("a decimal digit");
        let low = rn2 < m_dn_n;
        let high = &rn2 + &m_up_n > s_n;
        match (low, high) {
            (true, false) => {
                digits.push(dn);
                break;
            }
            (false, true) => {
                digits.push(dn + 1);
                break;
            }
            (true, true) => {
                digits.push(if &rn2 * two() < s_n { dn } else { dn + 1 });
                break;
            }
            (false, false) => {
                digits.push(dn);
                rn = rn2;
            }
        }
    }
    (digits, k)
}

impl std::fmt::Debug for F32Bits {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.to_f32())
    }
}

/// One element of a folded brace initializer.
///
/// Mirrors the two shapes the parser produces (`Parser.hs:857-864`) and the two
/// type constructors they get (`TPositional`, `TDesignator`; `Type.hs:239-241`).
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct AggregateElem {
    /// `Some(f)` for `.f = e`, `None` for a positional element.
    pub designator: Option<kg_base::Symbol>,
    pub value: ConstValue,
}

/// A fully folded compile-time constant.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum ConstValue {
    /// `IntLiteralF (Maybe LiteralType) Integer` (`Parser/Syntax.hs:347-350`).
    Int { ty: Option<LiteralType>, value: ConstInt },
    /// `BoolLiteralF Bool` (`Parser/Syntax.hs:354`).
    Bool(bool),
    /// `FloatLiteralF Float` (`Parser/Syntax.hs:352`) — always 32-bit
    /// (`Type.hs:247`).
    Float(F32Bits),
    /// `StringLiteralF String` (`Parser/Syntax.hs:356`). Width 0 (`Type.hs:705`).
    Str(SmolStr),
    /// `EnumValueF` — an enum constant that has folded to its underlying value.
    /// The path is the enum *constant*'s qualified name, so a later pass can
    /// still render `E::c` rather than the number.
    Enum { name: SymbolPath, value: ConstInt },
    /// A folded `InitializerListF` (`Parser/Syntax.hs:338`). Reached through the
    /// const-aggregate propagation of `Constexpr.hs:142-152` and required by the
    /// device-config path (`docs/spec/resolve.md` §9.3).
    Aggregate(Vec<AggregateElem>),
}

impl ConstValue {
    /// An untyped integer literal — the shape `untypedIntLiteral` produces
    /// (`Constexpr.hs:200-202`).
    pub fn untyped_int(value: impl Into<ConstInt>) -> ConstValue {
        ConstValue::Int { ty: None, value: value.into() }
    }

    pub fn typed_int(ty: LiteralType, value: impl Into<ConstInt>) -> ConstValue {
        ConstValue::Int { ty: Some(ty), value: value.into() }
    }

    /// The integer payload of an `Int` or `Enum`. `evalBinary` unwraps
    /// `EnumValue` before dispatching (`Constexpr.hs:173-174`), so most integer
    /// call sites want this rather than a match on `Int`.
    pub fn as_int(&self) -> Option<&ConstInt> {
        match self {
            ConstValue::Int { value, .. } | ConstValue::Enum { value, .. } => Some(value),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            ConstValue::Bool(b) => Some(*b),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            ConstValue::Str(s) => Some(s),
            _ => None,
        }
    }

    /// True for an integer literal with no `i`/`u` suffix. Drives the
    /// "result stays untyped" rule at `Constexpr.hs:200-202` and the minimal-type
    /// conversion check at `TypeCheck.hs:154`.
    pub fn is_untyped_int(&self) -> bool {
        matches!(self, ConstValue::Int { ty: None, .. })
    }

    /// A positive integer, as `fromNaturalConstexpr` demands (`Type.hs:1102-1103`).
    /// Array dimensions, `clog2` arguments and shift widths all go through this
    /// guard in the reference.
    pub fn as_natural(&self) -> Option<&ConstInt> {
        self.as_int().filter(|v| v.is_positive())
    }

    /// True for the four constructors `isLiteral` accepts (`Constexpr.hs:377-383`).
    /// **Excludes `Enum`** — that is `isEnum` (`Constexpr.hs:372-375`), a separate
    /// predicate — which is why `static(E::c)` does not unwrap
    /// (`Constexpr.hs:317-318`).
    pub fn is_literal(&self) -> bool {
        matches!(
            self,
            ConstValue::Int { .. } | ConstValue::Bool(_) | ConstValue::Float(_) | ConstValue::Str(_)
        )
    }

    /// `constexprResolved` (`Constexpr.hs:385-390`): a literal, an enum value, or
    /// an initializer list all of whose elements are themselves resolved.
    pub fn constexpr_resolved(&self) -> bool {
        match self {
            ConstValue::Aggregate(elems) => {
                elems.iter().all(|e| e.value.constexpr_resolved())
            }
            _ => true,
        }
    }

    /// The literal's own type as `inferIntLiteralType` computes it
    /// (`Type.hs:485-491`): the suffix when there is one, else
    /// [`LiteralType::minimal`] of the value.
    ///
    /// `None` for a non-integer. **Also `None` for [`ConstValue::Enum`]**: the
    /// reference asks `tEnumBase (typeOf x)` there (`Constexpr.hs:262`), which is
    /// a property of the enum *declaration* and not of the folded value — see
    /// [`crate::ConstEnv::enum_base`].
    pub fn literal_type(&self) -> Option<LiteralType> {
        match self {
            ConstValue::Int { ty: Some(t), .. } => Some(*t),
            ConstValue::Int { ty: None, value } => Some(LiteralType::minimal(value)),
            _ => None,
        }
    }

    /// `sizeOf` of an integer literal, i.e. `tWidth (inferIntLiteralType w x)`
    /// — the quantity `evalBinaryInt` compares in `x & 0` (`Constexpr.hs:231-232`).
    pub fn literal_width(&self) -> Option<u32> {
        self.literal_type().map(|t| t.width)
    }

    // --- token text -> value -------------------------------------------------

    /// Parses one `INT_NUMBER` token, including its optional `i<W>`/`u<W>` suffix.
    ///
    /// The token shape is `integer` (`Parser/Lexer.hs:133-155`) followed by
    /// `typeSuffix` (`Parser.hs:904-908`): an optional `0b`/`0o`/`0x` radix
    /// prefix (case-insensitive), an optional `_` immediately after it, then
    /// digits and `_` separators. `_` is dropped by `step` in `integerRadix`.
    ///
    /// The value is always **non-negative** — the reference has no negative
    /// integer literals; `-1` is `UnaryF Neg (IntLiteral Nothing 1)`
    /// (`Parser.hs:948`), folded by `Constexpr.hs:257-258`.
    pub fn parse_int_literal(text: &str) -> Option<ConstValue> {
        let (radix, rest) = match text.as_bytes() {
            [b'0', b'b' | b'B', rest @ ..] => (2, rest),
            [b'0', b'o' | b'O', rest @ ..] => (8, rest),
            [b'0', b'x' | b'X', rest @ ..] => (16, rest),
            all => (10, all),
        };
        let rest = std::str::from_utf8(rest).ok()?;
        // `prefix x = char '0' >> char' x >> optional (char '_')`.
        let rest = if radix == 10 { rest } else { rest.strip_prefix('_').unwrap_or(rest) };

        let digits_end = rest
            .bytes()
            .position(|b| !(b == b'_' || (b as char).is_digit(radix)))
            .unwrap_or(rest.len());
        let (digits, suffix) = rest.split_at(digits_end);
        let digits: String = digits.chars().filter(|c| *c != '_').collect();
        if digits.is_empty() {
            return None;
        }

        let ty = if suffix.is_empty() { None } else { Some(LiteralType::parse_suffix(suffix)?) };
        Some(ConstValue::Int { ty, value: ConstInt::parse(&digits, radix)? })
    }

    /// Parses one `FLOAT_NUMBER` token. `megaparsec`'s `L.float`
    /// (`Parser/Lexer.hs:158-159`) accepts exactly the shapes Rust's `f32`
    /// parser does for this grammar — `d+`, `d+.d+`, either with an optional
    /// `[eE][+-]?d+` — and no `_` separators.
    ///
    /// Held as bits: the front-end itself treats float constants as bit patterns
    /// (`Constexpr.hs:329-339`).
    pub fn parse_float_literal(text: &str) -> Option<ConstValue> {
        text.parse::<f32>().ok().map(|f| ConstValue::Float(F32Bits::from_f32(f)))
    }

    /// Decodes one `STRING` token — quotes included — into its value.
    ///
    /// `stringSegments` (`Parser.hs:885-893`) reads characters with
    /// `char '{' *> MP.charLiteral <|> MP.charLiteral`, so a doubled `{{`
    /// collapses to one `{` and every other character goes through megaparsec's
    /// Haskell-style `charLiteral`.
    ///
    /// This decoder covers the escapes a Kanagawa source can realistically
    /// contain — the single-character ones, `\xHH…`, `\oOO…` and `\DDD` — and
    /// passes an unrecognised escape through unchanged rather than failing.
    /// The full Haskell set (`\SOH`, `\^X`, `\&`) is not reproduced: string
    /// constants only ever participate in `==` / `!=` folding
    /// (`Constexpr.hs:212-215`), so the decoding matters solely for two
    /// *differently spelled* strings that should compare equal.
    pub fn parse_string_literal(text: &str) -> ConstValue {
        let body = text.strip_prefix('"').unwrap_or(text);
        let body = body.strip_suffix('"').unwrap_or(body);
        ConstValue::Str(SmolStr::new(decode_string(body)))
    }
}

/// The escape decoder behind [`ConstValue::parse_string_literal`].
fn decode_string(body: &str) -> String {
    let mut out = String::with_capacity(body.len());
    let mut chars = body.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '{' {
            // `char '{' *> MP.charLiteral`: a `{{` pair yields one `{`.
            if chars.peek() == Some(&'{') {
                chars.next();
            }
            out.push('{');
            continue;
        }
        if c != '\\' {
            out.push(c);
            continue;
        }
        let Some(e) = chars.next() else {
            out.push('\\');
            break;
        };
        let radix_escape = |chars: &mut std::iter::Peekable<std::str::Chars<'_>>, radix: u32| {
            let mut n: u32 = 0;
            while let Some(d) = chars.peek().and_then(|c| c.to_digit(radix)) {
                n = n.saturating_mul(radix).saturating_add(d);
                chars.next();
            }
            char::from_u32(n).unwrap_or('\u{fffd}')
        };
        match e {
            'a' => out.push('\u{7}'),
            'b' => out.push('\u{8}'),
            'f' => out.push('\u{c}'),
            'n' => out.push('\n'),
            'r' => out.push('\r'),
            't' => out.push('\t'),
            'v' => out.push('\u{b}'),
            '\\' => out.push('\\'),
            '"' => out.push('"'),
            '\'' => out.push('\''),
            '0'..='9' => {
                let mut n = e.to_digit(10).unwrap();
                while let Some(d) = chars.peek().and_then(|c| c.to_digit(10)) {
                    n = n.saturating_mul(10).saturating_add(d);
                    chars.next();
                }
                out.push(char::from_u32(n).unwrap_or('\u{fffd}'));
            }
            'x' | 'X' => out.push(radix_escape(&mut chars, 16)),
            'o' | 'O' => out.push(radix_escape(&mut chars, 8)),
            other => out.push(other),
        }
    }
    out
}

// --- operators ---------------------------------------------------------------
//
// These live here, not in `kg-syntax`, because the generated AST exposes only an
// untyped `op_token()` and because *two* downstream crates need the same enum:
// `kg-const` folds over them (`Constexpr.hs:176-250`) and `kg-ty` runs the
// bit-width algebra over them (`Type.hs:740-926`). `kg-ty` depends on `kg-const`,
// so this is the lowest crate that can hold one shared spelling.

/// `data BinaryOp` (`Parser/Syntax.hs:75-82`).
///
/// **Variant order is load-bearing**: `Parser/Syntax.hs:71-72` states the constructors
/// must match the C enum in `compiler/cpp/parse_tree.h`, and the Haskell derives
/// `Enum`. Do not reorder or insert.
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    /// `lutmul(a, b)` — a multiply the backend must implement in LUTs.
    LutMul,
    Div,
    Mod,
    BitwiseAnd,
    And,
    BitwiseOr,
    Or,
    BitwiseXor,
    Xor,
    ShiftLeft,
    ShiftRight,
    Equal,
    NotEqual,
    Greater,
    GreaterEqual,
    Less,
    LessEqual,
}

impl BinOp {
    /// Source spelling (`Parser/Syntax.hs:84-102`). `LutMul` has none — its
    /// `pretty` is `undefined` (`Parser/Syntax.hs:87`) because it is only ever
    /// written as the `lutmul(a, b)` intrinsic.
    pub fn text(self) -> Option<&'static str> {
        Some(match self {
            BinOp::Add => "+",
            BinOp::Sub => "-",
            BinOp::Mul => "*",
            BinOp::LutMul => return None,
            BinOp::Div => "/",
            BinOp::Mod => "%",
            BinOp::BitwiseAnd => "&",
            BinOp::And => "&&",
            BinOp::BitwiseOr => "|",
            BinOp::Or => "||",
            BinOp::BitwiseXor => "^",
            BinOp::Xor => "^^",
            BinOp::ShiftLeft => "<<",
            BinOp::ShiftRight => ">>",
            BinOp::Equal => "==",
            BinOp::NotEqual => "!=",
            BinOp::Greater => ">",
            BinOp::GreaterEqual => ">=",
            BinOp::Less => "<",
            BinOp::LessEqual => "<=",
        })
    }

    /// The six operators typed `TBoolean` with **no** operand check in the
    /// front-end (`Type.hs:790-795`; `docs/spec/types.md` §15 trap 30). The
    /// operand check happens only in the C++ middle-end, so a Rust front-end that
    /// diagnoses here fails the 241 backend-code assertions.
    pub fn is_comparison(self) -> bool {
        matches!(
            self,
            BinOp::Equal
                | BinOp::NotEqual
                | BinOp::Greater
                | BinOp::GreaterEqual
                | BinOp::Less
                | BinOp::LessEqual
        )
    }

    /// `&&`, `||`, `^^` — the boolean connectives.
    pub fn is_logical(self) -> bool {
        matches!(self, BinOp::And | BinOp::Or | BinOp::Xor)
    }
}

/// `data UnaryOp` (`Parser/Syntax.hs:107-113`). Order matches the C enum, as above.
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum UnOp {
    /// `~`
    Invert,
    /// `-`
    Neg,
    /// `!`
    Not,
    BitSizeof,
    ByteSizeof,
    Clog2,
}

impl UnOp {
    /// Source spelling (`Parser/Syntax.hs:116-122`).
    pub fn text(self) -> &'static str {
        match self {
            UnOp::Invert => "~",
            UnOp::Neg => "-",
            UnOp::Not => "!",
            UnOp::BitSizeof => "bitsizeof",
            UnOp::ByteSizeof => "bytesizeof",
            UnOp::Clog2 => "clog2",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enum_values_read_as_ints() {
        // Constexpr.hs:173-174 unwraps EnumValue before integer dispatch.
        let e = ConstValue::Enum {
            name: SymbolPath::empty(),
            value: ConstInt::from_i128(7),
        };
        assert_eq!(e.as_int(), Some(&ConstInt::from_i128(7)));
        assert!(!e.is_untyped_int());
    }

    #[test]
    fn natural_guard_rejects_zero_and_negative() {
        assert!(ConstValue::untyped_int(0i128).as_natural().is_none());
        assert!(ConstValue::untyped_int(-1i128).as_natural().is_none());
        assert!(ConstValue::untyped_int(1i128).as_natural().is_some());
    }

    fn i(v: i128) -> ConstInt {
        ConstInt::from_i128(v)
    }

    /// Every radix and separator form of `integer` (`Parser/Lexer.hs:133-155`),
    /// taken from the lexer's own corpus (`kg-parser/src/lexer/tests.rs:151-161`),
    /// plus the `typeSuffix` of `Parser.hs:904-908`.
    #[test]
    fn int_literal_tokens_parse() {
        let untyped = |v: i128| Some(ConstValue::Int { ty: None, value: i(v) });
        assert_eq!(ConstValue::parse_int_literal("0"), untyped(0));
        assert_eq!(ConstValue::parse_int_literal("42"), untyped(42));
        assert_eq!(ConstValue::parse_int_literal("305_441_741"), untyped(305_441_741));
        assert_eq!(ConstValue::parse_int_literal("0xff"), untyped(255));
        assert_eq!(ConstValue::parse_int_literal("0XFF"), untyped(255));
        assert_eq!(ConstValue::parse_int_literal("0x_1234_abcd"), untyped(0x1234_abcd));
        assert_eq!(ConstValue::parse_int_literal("0b_1111_0000"), untyped(0xf0));
        assert_eq!(ConstValue::parse_int_literal("0B0"), untyped(0));
        assert_eq!(ConstValue::parse_int_literal("0o777"), untyped(511));
        assert_eq!(ConstValue::parse_int_literal("0O_7_7"), untyped(63));
        // `test/syntax/constexpr.k:1180-1188` uses `0xffffffff` as a `uint32`.
        assert_eq!(ConstValue::parse_int_literal("0xffffffff"), untyped(0xffff_ffff));

        assert_eq!(
            ConstValue::parse_int_literal("42i32"),
            Some(ConstValue::typed_int(LiteralType::signed(32), i(42)))
        );
        assert_eq!(
            ConstValue::parse_int_literal("0xffu8"),
            Some(ConstValue::typed_int(LiteralType::unsigned(8), i(255)))
        );
        // `0xbeef` is all hex digits — the suffix scan must not eat into them.
        assert_eq!(ConstValue::parse_int_literal("0xbeef"), untyped(0xbeef));

        // Past `i128`: literals are `Integer` in the reference (`Syntax.hs:349`).
        let big = ConstValue::parse_int_literal(&format!("0x{}", "f".repeat(80))).unwrap();
        assert_eq!(big.as_int().unwrap().bits(), 320);
    }

    /// The `inferIntLiteralType Nothing` table (`Type.hs:485-491`), with the
    /// values `docs/spec/types.md` §3.1 validated against
    /// `test/syntax/literal-operand.k:69-79`, cross-checked against the
    /// `bitsizeof` assertions of `test/syntax/constexpr.k:434-479`.
    #[test]
    fn minimal_literal_type_is_infer_int_literal_types_nothing_branch() {
        let u = LiteralType::unsigned;
        let s = LiteralType::signed;
        assert_eq!(LiteralType::minimal(&i(0)), u(1));
        assert_eq!(LiteralType::minimal(&i(1)), u(1));
        assert_eq!(LiteralType::minimal(&i(2)), u(2));
        assert_eq!(LiteralType::minimal(&i(3)), u(2));
        assert_eq!(LiteralType::minimal(&i(4)), u(3));
        assert_eq!(LiteralType::minimal(&i(10)), u(4));
        assert_eq!(LiteralType::minimal(&i(255)), u(8));
        assert_eq!(LiteralType::minimal(&i(256)), u(9));
        assert_eq!(LiteralType::minimal(&i(-1)), s(1));
        assert_eq!(LiteralType::minimal(&i(-10)), s(5));
        assert_eq!(LiteralType::minimal(&i(-16)), s(5));
        assert_eq!(LiteralType::minimal(&i(-17)), s(6));
        assert_eq!(LiteralType::minimal(&i(-20)), s(6));
        assert_eq!(LiteralType::minimal(&i(-200)), s(9));
    }

    /// `tSigned` (`Type.hs:550-551`) — `uintW` promotes to `int(W+1)`, `intW`
    /// stays put. Reached from `evalUnary Invert` on an untyped literal.
    #[test]
    fn to_signed_matches_t_signed() {
        assert_eq!(LiteralType::unsigned(5).to_signed(), LiteralType::signed(6));
        assert_eq!(LiteralType::signed(5).to_signed(), LiteralType::signed(5));
        // `~0`: minimal(0) = uint1, tSigned → int2, and `truncateInt 2 (-1)` is -1.
        assert_eq!(LiteralType::minimal(&i(0)).to_signed(), LiteralType::signed(2));
    }

    #[test]
    fn literal_type_prefers_the_suffix() {
        let suffixed = ConstValue::typed_int(LiteralType::unsigned(32), i(10));
        assert_eq!(suffixed.literal_type(), Some(LiteralType::unsigned(32)));
        assert_eq!(suffixed.literal_width(), Some(32));
        // Unsuffixed: re-derived from the value every time.
        assert_eq!(ConstValue::untyped_int(10i128).literal_width(), Some(4));
        // An enum's underlying suffix is not carried by the value domain.
        let e = ConstValue::Enum { name: SymbolPath::empty(), value: i(1) };
        assert_eq!(e.literal_type(), None);
    }

    /// `isLiteral` (`Constexpr.hs:377-383`) lists exactly four constructors and
    /// `EnumValue` is not one of them, which is why `static(E::c)` does not
    /// unwrap (`Constexpr.hs:317-318`).
    #[test]
    fn is_literal_excludes_enum_and_aggregate() {
        assert!(ConstValue::untyped_int(1i128).is_literal());
        assert!(ConstValue::Bool(true).is_literal());
        assert!(ConstValue::Float(F32Bits::from_f32(1.0)).is_literal());
        assert!(ConstValue::Str(SmolStr::new_static("x")).is_literal());
        assert!(!ConstValue::Enum { name: SymbolPath::empty(), value: i(1) }.is_literal());
        assert!(!ConstValue::Aggregate(Vec::new()).is_literal());
    }

    /// `constexprResolved` (`Constexpr.hs:385-390`) descends into an initializer
    /// list; everything else is resolved by construction, because a
    /// [`ConstValue`] only exists once its node folded.
    #[test]
    fn constexpr_resolved_descends_into_aggregates() {
        let ok = ConstValue::Aggregate(vec![AggregateElem {
            designator: None,
            value: ConstValue::untyped_int(1i128),
        }]);
        assert!(ok.constexpr_resolved());
        assert!(ConstValue::Aggregate(Vec::new()).constexpr_resolved());
    }

    #[test]
    fn float_literals_are_bit_patterns() {
        // `test/syntax/constexpr.k:1449-1454` asserts
        // `cast<float32>(0x40200000) == 2.5`, so 2.5 must be exactly those bits.
        assert_eq!(
            ConstValue::parse_float_literal("2.5"),
            Some(ConstValue::Float(F32Bits(0x4020_0000)))
        );
        // `test/syntax/constexpr.k:604-609`: `bitsizeof 3.14 == 32`.
        assert!(matches!(ConstValue::parse_float_literal("3.14"), Some(ConstValue::Float(_))));
        assert!(ConstValue::parse_float_literal("1.5e10").is_some());
        assert!(ConstValue::parse_float_literal("1e5").is_some());
    }

    /// String values exist only so that `==` / `!=` can fold
    /// (`Constexpr.hs:212-215`); the decoder has to agree with itself on two
    /// spellings of the same text.
    #[test]
    fn string_literals_decode() {
        let s = |t: &str| ConstValue::parse_string_literal(t).as_str().unwrap().to_owned();
        assert_eq!(s("\"abc\""), "abc");
        assert_eq!(s("\"a\\nb\""), "a\nb");
        assert_eq!(s("\"\\x41\""), "A");
        assert_eq!(s("\"\\65\""), "A");
        assert_eq!(s("\"\\\\\""), "\\");
        // `char '{' *> MP.charLiteral` collapses the `{{` escape
        // (`Parser.hs:889-891`).
        assert_eq!(s("\"a{{b\""), "a{b");
        assert_eq!(
            ConstValue::parse_string_literal("\"\\x41\""),
            ConstValue::parse_string_literal("\"A\"")
        );
    }

    #[test]
    fn literal_suffix_round_trips() {
        for text in ["i1", "u1", "i32", "u8", "i1024"] {
            let t = LiteralType::parse_suffix(text).unwrap();
            assert_eq!(t.to_string(), text);
        }
        assert_eq!(LiteralType::parse_suffix(""), None);
        assert_eq!(LiteralType::parse_suffix("i"), None);
        assert_eq!(LiteralType::parse_suffix("x8"), None);
    }

    /// Read off GHC 9.6's `show :: Float` (the reference's compiler): the
    /// fixed/exponent switch at `0.1` and `10^7`, a denormal, the sign of
    /// zero — and `2^25 * 1.5`, where Rust's shortest round trip is
    /// `5.033165e7` and GHC's strict boundary keeps a digit more (1,171 of
    /// 544,923 bit patterns differ so; every one agreed with this port,
    /// measured 2026-09-27 against a compiled GHC sweep).
    #[test]
    fn a_float_is_shown_as_haskell_shows_it() {
        let cases: &[(u32, &str)] = &[
            (1_036_831_949, "0.1"),
            (1_008_981_770, "1.0e-2"),
            (1_259_902_592, "1.0e7"),
            (1_234_613_304, "1234567.0"),
            (1_262_248_270, "1.2345678e7"),
            (3_213_675_725, "-1.1"),
            (1_120_403_456, "100.0"),
            (2_147_483_648, "-0.0"),
            (0, "0.0"),
            (1_065_353_216, "1.0"),
            (1_056_964_608, "0.5"),
            (2_139_095_039, "3.4028235e38"),
            (2_139_095_040, "Infinity"),
            (4_286_578_688, "-Infinity"),
            (1, "1.0e-45"),
            (8_388_608, "1.1754944e-38"),
            (1_279_262_720, "5.0331648e7"),
            (1_287_651_328, "1.00663296e8"),
        ];
        for &(bits, shown) in cases {
            assert_eq!(F32Bits(bits).haskell_show(), shown, "{bits}");
        }
    }
}
