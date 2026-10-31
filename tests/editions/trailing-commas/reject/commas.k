// One comma closes a list; two do not (`docs/editions/1.1.md`, "Trailing
// commas"). The parser recovers past the second comma and reports what it
// expected at each step.
//
// expect-error 12:23: expected a name
// expect-error 12:23: expected an enum constant
// expect-error 12:23: expected `}`
// expect-error 12:23: expected a declaration
// expect-error 12:25: expected a declaration
// expect-error 12:25: expected a declaration

enum E : uint2 { A, B,, }
