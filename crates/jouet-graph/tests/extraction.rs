//! Ports of codegraph's pure-extraction tests (`__tests__/extraction.test.ts`
//! and the kernel-*-parity suites) for the languages jouet extracts. Each
//! test names the codegraph `describe` / `it` it came from. codegraph's
//! `unresolvedReferences` are [`Ref`]s here, and node IDs in edges and refs
//! are indexes into [`Extraction::nodes`].

use jouet_graph::extract::{Extraction, Node, Ref, detect_language, extract, language_for_path};

fn ex(path: &str, src: &str) -> Extraction {
    extract(path, src)
        .unwrap_or_else(|| panic!("{path}: no extractor"))
        .unwrap_or_else(|e| panic!("{path}: {e}"))
}

fn find<'a>(x: &'a Extraction, kind: &str, name: &str) -> Option<&'a Node> {
    x.nodes.iter().find(|n| n.kind == kind && n.name == name)
}

fn node<'a>(x: &'a Extraction, kind: &str, name: &str) -> &'a Node {
    find(x, kind, name).unwrap_or_else(|| {
        let all: Vec<String> = x
            .nodes
            .iter()
            .map(|n| format!("{}:{}", n.kind, n.name))
            .collect();
        panic!("no {kind} {name} in {all:?}")
    })
}

/// The first node of any kind with this name.
fn named<'a>(x: &'a Extraction, name: &str) -> Option<&'a Node> {
    x.nodes.iter().find(|n| n.name == name)
}

fn of_kind<'a>(x: &'a Extraction, kind: &'a str) -> impl Iterator<Item = &'a Node> + 'a {
    x.nodes.iter().filter(move |n| n.kind == kind)
}

/// Names of the nodes of a kind, in extraction order.
fn names(x: &Extraction, kind: &str) -> Vec<String> {
    of_kind(x, kind).map(|n| n.name.clone()).collect()
}

fn sorted(mut v: Vec<String>) -> Vec<String> {
    v.sort();
    v
}

fn refs<'a>(x: &'a Extraction, kind: &str) -> Vec<&'a Ref> {
    x.refs.iter().filter(|r| r.kind == kind).collect()
}

fn ref_names(x: &Extraction, kind: &str) -> Vec<String> {
    refs(x, kind).iter().map(|r| r.name.clone()).collect()
}

fn has_ref(x: &Extraction, kind: &str, name: &str) -> bool {
    x.refs.iter().any(|r| r.kind == kind && r.name == name)
}

fn idx(x: &Extraction, n: &Node) -> usize {
    x.nodes
        .iter()
        .position(|m| std::ptr::eq(m, n))
        .expect("node of this extraction")
}

fn owner<'a>(x: &'a Extraction, r: &Ref) -> &'a Node {
    &x.nodes[r.from]
}

/// Names of the refs of a kind made from `n`, in order.
fn refs_from(x: &Extraction, n: &Node, kind: &str) -> Vec<String> {
    let i = idx(x, n);
    refs(x, kind)
        .iter()
        .filter(|r| r.from == i)
        .map(|r| r.name.clone())
        .collect()
}

fn contains(x: &Extraction, source: &Node, target: &Node) -> bool {
    let (s, t) = (idx(x, source), idx(x, target));
    x.edges
        .iter()
        .any(|e| e.kind == "contains" && e.source == s && e.target == t)
}

fn sig(n: &Node) -> &str {
    n.signature.as_deref().unwrap_or("")
}

/// The first import node of a one-file extraction.
fn first_import(path: &str, src: &str) -> Node {
    let x = ex(path, src);
    of_kind(&x, "import")
        .next()
        .unwrap_or_else(|| panic!("{path}: no import"))
        .clone()
}

fn crlf(s: &str) -> String {
    s.replace('\n', "\r\n")
}

/// Length in UTF-16 code units: codegraph's column unit.
fn utf16_len(s: &str) -> u32 {
    s.encode_utf16().count() as u32
}

// ---------------------------------------------------------------------------
// Language detection
// ---------------------------------------------------------------------------

// Language Detection: should detect TypeScript / JavaScript / Python / Go /
// Rust / Java / C / C++ / CUDA files; should return unknown for unsupported
// extensions; Regression: .mts/.cts (#366, #556).
#[test]
fn detects_languages_by_extension() {
    let cases = [
        ("src/index.ts", "typescript"),
        ("components/Button.tsx", "tsx"),
        ("index.js", "javascript"),
        ("App.jsx", "jsx"),
        ("config.mjs", "javascript"),
        ("main.py", "python"),
        ("main.go", "go"),
        ("lib.rs", "rust"),
        ("Main.java", "java"),
        ("main.c", "c"),
        ("utils.h", "c"),
        ("main.cpp", "cpp"),
        ("class.hpp", "cpp"),
        ("kernels/scan.cu", "cpp"),
        ("include/reduce.cuh", "cpp"),
        ("mod.mts", "typescript"),
        ("mod.cts", "typescript"),
    ];
    for (path, lang) in cases {
        assert_eq!(detect_language(path, ""), Some(lang), "{path}");
        assert_eq!(language_for_path(path), Some(lang), "{path}");
    }
    assert_eq!(detect_language("styles.css", ""), None);
    assert_eq!(detect_language("data.json", ""), None);
}

// Language Detection: should detect a .h whose only C++ signal is an
// export-macro class as cpp
#[test]
fn detects_export_macro_header_as_cpp() {
    let macro_class_header = "#pragma once\n#include \"CoreMinimal.h\"\n\nUCLASS()\nclass ENGINE_API UNetConnectionRepControl : public UObject\n{\n\tGENERATED_BODY()\n\tbool IsRepControlEnable() const;\n};\n";
    assert_eq!(
        detect_language("NetConnectionRepControl.h", macro_class_header),
        Some("cpp")
    );
    assert_eq!(
        detect_language(
            "Foo.h",
            "MYMODULE_API_DECL\nclass MYMODULE_API FFoo\n{\n\tint X;\n};\n"
        ),
        Some("cpp")
    );
    assert_eq!(
        detect_language("Bar.h", "struct ENGINE_API FBar : public FBase {};\n"),
        Some("cpp")
    );
    assert_eq!(
        detect_language(
            "cfoo.h",
            "#ifndef CFOO_H\nstruct Point { int x; int y; };\nvoid f(struct Point p);\n#endif\n"
        ),
        Some("c")
    );
}

// Language Detection: should detect a .h whose only C++ signal is a plain
// base clause as cpp (#1592)
#[test]
fn detects_plain_base_clause_header_as_cpp() {
    for (path, src) in [
        ("min.h", "struct Base {};\nstruct Derived : Base {};\n"),
        ("pub.h", "struct Derived : public Base {};\n"),
        ("scoped.h", "struct Derived : ns::Base {};\n"),
        ("tmpl.h", "struct Derived : Base<int, Foo<T>> {};\n"),
        ("final.h", "struct Derived final : Base {};\n"),
        ("multi.h", "class Derived : public A, private B\n{\n};\n"),
        ("virt.h", "struct Derived : virtual Base {};\n"),
    ] {
        assert_eq!(detect_language(path, src), Some("cpp"), "{path}");
    }
    let preamble = format!(
        "#ifndef BIG_H\n#define BIG_H\n{}",
        "#define VALUE_0 0\n".repeat(700)
    );
    assert!(preamble.len() > 8192);
    assert_eq!(
        detect_language(
            "big.h",
            &format!("{preamble}struct Base {{}};\nstruct Derived : Base {{}};\n#endif\n")
        ),
        Some("cpp")
    );
    for (path, src) in [
        (
            "bits.h",
            "struct S { unsigned int a : 3; unsigned int b : 5; };\n",
        ),
        (
            "tern.h",
            "static inline int sz(int x) { return x ? sizeof(struct foo) : 0; }\n#define P(a,b) ((a) ? (struct foo *)(a) : (b))\n",
        ),
        (
            "label.h",
            "static void g(void) {\nstruct_end:\n  return;\n}\nint struct_a, struct_b;\n",
        ),
        (
            "doc.h",
            "/* struct timeval: seconds, microseconds */\nstruct timeval { long tv_sec; long tv_usec; };\n// struct foo: x, y\n",
        ),
        (
            "cfoo.h",
            "#ifndef CFOO_H\nstruct Point { int x; int y; };\nvoid f(struct Point p);\n#endif\n",
        ),
        ("stdio.h", "#ifndef STDIO_H\nvoid printf();\n#endif\n"),
    ] {
        assert_eq!(detect_language(path, src), Some("c"), "{path}");
    }
}

// Language Detection: should extract a derived struct from a plain
// base-clause .h, with no phantom function (#1592)
#[test]
fn extracts_derived_struct_from_base_clause_header() {
    let src = "struct Base {};\nstruct Derived : Base {};\n";
    let x = ex("src/min.h", src);
    let derived = named(&x, "Derived").expect("Derived");
    assert_eq!(derived.kind, "struct");
    assert_eq!(detect_language("src/min.h", src), Some("cpp"));
    assert!(find(&x, "function", "Base").is_none());
    let bases: Vec<&Node> = x.nodes.iter().filter(|n| n.name == "Base").collect();
    assert_eq!(bases.len(), 1);
    assert_eq!(bases[0].kind, "struct");
}

// ---------------------------------------------------------------------------
// TypeScript / JavaScript
// ---------------------------------------------------------------------------

// TypeScript Extraction: should extract function declarations
#[test]
fn ts_function_declarations() {
    let x = ex(
        "payment.ts",
        r#"
export function processPayment(amount: number): Promise<Receipt> {
  return stripe.charge(amount);
}
"#,
    );
    let file = of_kind(&x, "file").next().expect("file node");
    assert_eq!(file.name, "payment.ts");
    let func = of_kind(&x, "function").next().expect("function");
    assert_eq!(func.name, "processPayment");
    assert!(func.is_exported);
    assert_eq!(detect_language("payment.ts", ""), Some("typescript"));
    assert!(sig(func).contains("amount: number"));
}

// TypeScript Extraction: should extract class declarations
#[test]
fn ts_class_declarations() {
    let x = ex(
        "service.ts",
        r#"
export class PaymentService {
  private stripe: StripeClient;

  constructor(apiKey: string) {
    this.stripe = new StripeClient(apiKey);
  }

  async charge(amount: number): Promise<Receipt> {
    return this.stripe.charge(amount);
  }
}
"#,
    );
    let class = of_kind(&x, "class").next().expect("class");
    assert_eq!(class.name, "PaymentService");
    assert!(class.is_exported);
    assert!(!names(&x, "method").is_empty());
    assert!(find(&x, "method", "charge").is_some());
}

// TypeScript Extraction: captures docstrings for export- and const-wrapped
// declarations (#780)
#[test]
fn ts_docstrings_for_wrapped_declarations() {
    let x = ex(
        "doc.ts",
        r#"
// plain class control
class Ledger {}

// exported class
export class Invoice {}

// export default
export default function settle() { return true; }

// exported arrow const
export const refund = (amount: number) => amount;

// non-export arrow const
const audit = (amount: number) => amount;
"#,
    );
    let doc = |name: &str| named(&x, name).and_then(|n| n.docstring.clone());
    assert_eq!(doc("Ledger").as_deref(), Some("plain class control"));
    assert_eq!(doc("Invoice").as_deref(), Some("exported class"));
    assert_eq!(doc("settle").as_deref(), Some("export default"));
    assert_eq!(doc("refund").as_deref(), Some("exported arrow const"));
    assert_eq!(doc("audit").as_deref(), Some("non-export arrow const"));
}

// TypeScript Extraction: does not mis-attribute a class comment to an
// uncommented member (#780)
#[test]
fn ts_class_comment_not_given_to_member() {
    let x = ex(
        "box.ts",
        r#"
// Comment for Box
export class Box {
  noComment() {}
  // own comment
  withComment() {}
}
"#,
    );
    let doc = |name: &str| named(&x, name).and_then(|n| n.docstring.clone());
    assert_eq!(doc("Box").as_deref(), Some("Comment for Box"));
    assert_eq!(doc("noComment"), None);
    assert_eq!(doc("withComment").as_deref(), Some("own comment"));
}

// TypeScript Extraction: cleans comment markers across language styles
// (#780) -- the Rust and C cases
#[test]
fn comment_markers_cleaned_rust_and_c() {
    let doc = |file: &str, code: &str, name: &str| {
        let x = ex(file, code);
        named(&x, name).and_then(|n| n.docstring.clone())
    };
    assert_eq!(
        doc("m.rs", "/// rust doc line\nfn rs_fn() {}", "rs_fn").as_deref(),
        Some("rust doc line")
    );
    // Past the attributes between the doc comment and its item, and not a
    // comment a blank line separates from it.
    assert_eq!(
        doc(
            "m.rs",
            "//! The module.\n\n/// A unit.\npub struct Unit;",
            "Unit"
        )
        .as_deref(),
        Some("A unit.")
    );
    assert_eq!(doc("m.rs", "// Licence.\n\npub struct Bare;", "Bare"), None);
    assert_eq!(
        doc(
            "m.rs",
            "/// A point.\n#[derive(Clone)]\n#[cfg_attr(test, derive(Debug))]\npub struct Point {\n    x: u8,\n}",
            "Point"
        )
        .as_deref(),
        Some("A point.")
    );
    assert_eq!(
        doc("m.c", "/* c block */\nvoid c_fn(void) {}", "c_fn").as_deref(),
        Some("c block")
    );
}

// TypeScript Extraction: should extract interfaces
#[test]
fn ts_interfaces() {
    let x = ex(
        "types.ts",
        r#"
export interface User {
  id: string;
  name: string;
  email: string;
}
"#,
    );
    assert!(of_kind(&x, "file").next().is_some());
    let iface = of_kind(&x, "interface").next().expect("interface");
    assert_eq!(iface.name, "User");
    assert!(iface.is_exported);
}

// TypeScript Extraction: should extract type references from interface
// property signatures
#[test]
fn ts_interface_property_type_refs() {
    let x = ex(
        "HeaderFilter.ts",
        r#"
import type { IPage } from '../PromoterList';
import type { IOrderField } from '../types';

interface Hprops {
  value?: Partial<IPage> & Partial<IOrderField>;
}
"#,
    );
    assert!(has_ref(&x, "references", "IPage"));
    assert!(has_ref(&x, "references", "IOrderField"));
}

// TypeScript Extraction: indexes interface members, not just the interface
// itself
#[test]
fn ts_interface_members_indexed() {
    let x = ex(
        "api.d.ts",
        r#"
export interface PlatformApi {
  fetchPage(id: string): Promise<string>;
  version: string;
}
"#,
    );
    let iface = node(&x, "interface", "PlatformApi");
    let method = node(&x, "method", "fetchPage");
    let prop = node(&x, "property", "version");
    assert!(contains(&x, iface, method));
    assert!(contains(&x, iface, prop));
}

// TypeScript Extraction: does not mint a top-level function from a type
// literal method signature
#[test]
fn ts_type_literal_method_is_not_a_function() {
    let x = ex(
        "t.ts",
        r#"
export type Handle = { stop(): void; label: string };
"#,
    );
    node(&x, "type_alias", "Handle");
    node(&x, "method", "stop");
    assert!(find(&x, "function", "stop").is_none());
}

// TypeScript Extraction: should extract type references from interface
// method signatures
#[test]
fn ts_interface_method_signature_type_refs() {
    let x = ex(
        "MethodForm.ts",
        r#"
import type { IPage } from '../PromoterList';
import type { IOrderField } from '../types';

interface MethodForm {
  fetchPage(arg: IPage): IOrderField;
}
"#,
    );
    assert!(has_ref(&x, "references", "IPage"));
    assert!(has_ref(&x, "references", "IOrderField"));
}

// TypeScript Extraction: extracts type references from in-body local
// variable annotations
#[test]
fn ts_in_body_local_annotation_refs() {
    let x = ex(
        "inbody.ts",
        r#"
import { Foo } from './types';

export function build(): void {
  const items: Foo[] = [];
  void items;
}

export class K {
  run(): void {
    const a: Foo = { x: 1 };
    void a;
  }
}

export const handler = {
  handle(): void {
    const b: Foo = { x: 1 };
    void b;
  },
};
"#,
    );
    let foo: Vec<&Ref> = refs(&x, "references")
        .into_iter()
        .filter(|r| r.name == "Foo")
        .collect();
    assert!(foo.len() >= 3, "{foo:?}");
    for r in foo {
        let kind = owner(&x, r).kind;
        assert!(kind == "function" || kind == "method", "{kind}");
    }
    assert!(
        !x.nodes
            .iter()
            .any(|n| ["items", "a", "b"].contains(&n.name.as_str()))
    );
}

// TypeScript Extraction: should track function calls
#[test]
fn ts_tracks_function_calls() {
    let x = ex(
        "main.ts",
        r#"
function main() {
  const result = processData();
  console.log(result);
}
"#,
    );
    assert!(!x.refs.is_empty());
    assert!(has_ref(&x, "calls", "processData"));
}

// TypeScript Extraction: an awaited call with type arguments names its
// callee: %s (#2586)
#[test]
fn ts_awaited_generic_call_names_callee() {
    let code = [
        r#"export async function awaited() { return await fetchApi<User>("/me"); }"#,
        r#"export async function assigned() { const u = await fetchApi<User>("/me"); return u; }"#,
        r#"export async function viaNamespace() { return await api.fetchApi<User>("/me"); }"#,
        r#"export async function viaThis() { return await this.load<User>("/me"); }"#,
        "",
    ]
    .join("\n");
    for file in ["user.ts", "user.tsx"] {
        let x = ex(file, &code);
        let by_fn = |name: &str| refs_from(&x, node(&x, "function", name), "calls");
        assert_eq!(by_fn("awaited"), ["fetchApi"], "{file}");
        assert_eq!(by_fn("assigned"), ["fetchApi"], "{file}");
        assert_eq!(by_fn("viaNamespace"), ["api.fetchApi"], "{file}");
        assert_eq!(by_fn("viaThis"), ["load"], "{file}");
        assert!(
            !ref_names(&x, "calls")
                .iter()
                .any(|c| c.starts_with("await"))
        );
        let lines: Vec<&str> = code.split('\n').collect();
        let column_of = |name: &str| {
            let i = idx(&x, node(&x, "function", name));
            refs(&x, "calls")
                .iter()
                .find(|r| r.from == i)
                .unwrap()
                .column
        };
        assert_eq!(
            column_of("awaited") as usize,
            lines[0].find("fetchApi").unwrap()
        );
        assert_eq!(
            column_of("viaNamespace") as usize,
            lines[2].find("api.fetchApi").unwrap()
        );
    }
}

// TypeScript Extraction: an awaited generic call peels every await and skips
// comments: %s (#2586)
#[test]
fn ts_awaited_generic_call_peels_awaits_and_comments() {
    let code = [
        r#"export async function chainedParen() { return (await fetchApi<User>("/me")).json(); }"#,
        r#"export async function chainedThen() { return await api.get<User>("/me").then(r => r); }"#,
        r#"export async function commented() { return await /* typed */ fetchApi<User>("/me"); }"#,
        "export async function lineCommented() { return await // typed\n fetchApi<User>(\"/me\"); }",
        r#"export async function doubled() { return await await fetchApi<User>("/me"); }"#,
        "",
    ]
    .join("\n");
    for file in ["user.ts", "user.tsx"] {
        let x = ex(file, &code);
        let by_fn = |name: &str| sorted(refs_from(&x, node(&x, "function", name), "calls"));
        assert_eq!(by_fn("commented"), ["fetchApi"], "{file}");
        assert_eq!(by_fn("lineCommented"), ["fetchApi"], "{file}");
        assert_eq!(by_fn("doubled"), ["fetchApi"], "{file}");
        assert!(
            !ref_names(&x, "calls")
                .iter()
                .any(|c| c.starts_with("await") || c.starts_with("/*") || c.starts_with("//"))
        );
        assert_eq!(
            by_fn("chainedParen"),
            ["fetchApi", "fetchApi().json"],
            "{file}"
        );
        assert_eq!(
            by_fn("chainedThen"),
            ["api.get", "api.get().then"],
            "{file}"
        );
    }
}

// Arrow Function Export Extraction: should extract exported arrow functions
// assigned to const
#[test]
fn ts_exported_arrow_function() {
    let x = ex(
        "hooks.ts",
        r#"
export const useAuth = (): AuthContextValue => {
  return useContext(AuthContext);
};
"#,
    );
    assert!(node(&x, "function", "useAuth").is_exported);
}

// Arrow Function Export Extraction: should extract exported function
// expressions assigned to const
#[test]
fn ts_exported_function_expression() {
    let x = ex(
        "utils.ts",
        r#"
export const processData = function(input: string): string {
  return input.trim();
};
"#,
    );
    assert!(node(&x, "function", "processData").is_exported);
}

// Arrow Function Export Extraction: should not extract non-exported arrow
// functions as exported
#[test]
fn ts_non_exported_arrow_function() {
    let x = ex(
        "internal.ts",
        r#"
const internalHelper = () => {
  return 42;
};
"#,
    );
    assert!(
        !named(&x, "internalHelper")
            .expect("internalHelper")
            .is_exported
    );
}

// Arrow Function Export Extraction: should still skip truly anonymous arrow
// functions
#[test]
fn ts_anonymous_arrow_skipped() {
    let x = ex(
        "anon.ts",
        r#"
const items = [1, 2, 3].map((x) => x * 2);
"#,
    );
    assert!(find(&x, "function", "<anonymous>").is_none());
}

// Arrow Function Export Extraction: should extract multiple exported arrow
// functions from the same file
#[test]
fn ts_multiple_exported_arrows() {
    let x = ex(
        "math.ts",
        r#"
export const add = (a: number, b: number): number => a + b;

export const subtract = (a: number, b: number): number => a - b;

const internal = () => 'not exported';
"#,
    );
    let exported: Vec<String> = of_kind(&x, "function")
        .filter(|n| n.is_exported)
        .map(|n| n.name.clone())
        .collect();
    assert_eq!(sorted(exported), ["add", "subtract"]);
    assert!(!named(&x, "internal").expect("internal").is_exported);
}

// Arrow Function Export Extraction: should extract arrow functions in
// JavaScript files
#[test]
fn js_exported_arrow_function() {
    let x = ex(
        "api.js",
        r#"
export const fetchData = async () => {
  const response = await fetch('/api/data');
  return response.json();
};
"#,
    );
    assert!(node(&x, "function", "fetchData").is_exported);
}

// Generator Function Extraction (#1741): extracts function* and async
// function* declarations in TypeScript / JavaScript
#[test]
fn generator_declarations() {
    let code = r#"
function plain() { return 1; }
function* gen() { yield 2; }
async function asyncFn() { return 3; }
async function* asyncGen() { yield 4; }
"#;
    for file in ["gens.ts", "gens.js"] {
        assert_eq!(
            sorted(names(&ex(file, code), "function")),
            ["asyncFn", "asyncGen", "gen", "plain"],
            "{file}"
        );
    }
}

// Generator Function Extraction (#1741): extracts const-assigned generator
// and async generator expressions (TS) / (JS)
#[test]
fn generator_expressions() {
    let code = r#"
const g = function* () { yield 1; };
const ag = async function* () { yield 2; };
export const exportedGen = function* () { yield 3; };
"#;
    for file in ["gen-expr.ts", "gen-expr.js"] {
        let x = ex(file, code);
        assert_eq!(
            sorted(names(&x, "function")),
            ["ag", "exportedGen", "g"],
            "{file}"
        );
        assert!(named(&x, "exportedGen").unwrap().is_exported);
        assert!(!named(&x, "g").unwrap().is_exported);
    }
}

// Type Alias Extraction: should extract exported type aliases in TypeScript
#[test]
fn ts_exported_type_alias() {
    let x = ex(
        "types.ts",
        r#"
export type AuthContextValue = {
  user: User | null;
  login: () => void;
  logout: () => void;
};
"#,
    );
    let alias = of_kind(&x, "type_alias").next().expect("type_alias");
    assert_eq!(alias.name, "AuthContextValue");
    assert!(alias.is_exported);
}

// Type Alias Extraction: should extract non-exported type aliases
#[test]
fn ts_non_exported_type_alias() {
    let x = ex(
        "internal.ts",
        r#"
type InternalState = {
  loading: boolean;
  error: string | null;
};
"#,
    );
    let alias = of_kind(&x, "type_alias").next().expect("type_alias");
    assert_eq!(alias.name, "InternalState");
    assert!(!alias.is_exported);
}

// Type Alias Extraction: should extract multiple type aliases from the same
// file
#[test]
fn ts_multiple_type_aliases() {
    let x = ex(
        "config.ts",
        r#"
export type UnitSystem = 'metric' | 'imperial';
export type DateFormat = 'ISO' | 'US' | 'EU';
type Internal = string;
"#,
    );
    assert_eq!(of_kind(&x, "type_alias").count(), 3);
    let exported: Vec<String> = of_kind(&x, "type_alias")
        .filter(|n| n.is_exported)
        .map(|n| n.name.clone())
        .collect();
    assert_eq!(sorted(exported), ["DateFormat", "UnitSystem"]);
}

// Type Alias Extraction: extracts string-literal contract names from a
// generic tuple type alias (#634)
#[test]
fn ts_tuple_alias_contract_names() {
    let x = ex(
        "services/api.ts",
        r#"
interface Service<Name extends string, Req, Resp> { name: Name; }
export type MyServiceList = [
  Service<'query_apply_record', { pageNo: number }, { ok: boolean }>,
  Service<'apply_confirm', { code: string }, { ok: boolean }>
];
"#,
    );
    let contract: Vec<&Node> = of_kind(&x, "method")
        .filter(|n| n.qualified_name.starts_with("MyServiceList::"))
        .collect();
    assert_eq!(
        sorted(contract.iter().map(|n| n.name.clone()).collect()),
        ["apply_confirm", "query_apply_record"]
    );
    let query = node(&x, "method", "query_apply_record");
    assert_eq!(query.qualified_name, "MyServiceList::query_apply_record");
    assert!(sig(query).contains("Service<'query_apply_record'"));
    let alias = node(&x, "type_alias", "MyServiceList");
    assert!(contains(&x, alias, query));
}

// Type Alias Extraction: does not extract string literals from utility types
// or nested generics (#634)
#[test]
fn ts_no_literals_from_utility_types() {
    let x = ex(
        "noise.ts",
        r#"
interface User { id: string; name: string; }
interface Service<Name extends string, Req, Resp> { name: Name; }
export type Picked = Pick<User, 'id' | 'name'>;
export type Rec = Record<'foo' | 'bar', number>;
// Tuple entry, but the name is a non-identifier route path; the nested Pick's
// 'id' must also stay out (only DIRECT literal args of a tuple's generic count).
export type Routes = [Service<'/api/users', Pick<User, 'id'>, {}>];
// Bare string-literal tuple — not generic type arguments.
export type Names = ['alpha', 'beta'];
"#,
    );
    let ifaces: Vec<usize> = (0..x.nodes.len())
        .filter(|&i| x.nodes[i].kind == "interface")
        .collect();
    let declared: Vec<usize> = x
        .edges
        .iter()
        .filter(|e| e.kind == "contains" && ifaces.contains(&e.source))
        .map(|e| e.target)
        .collect();
    let leaked: Vec<&Node> = x
        .nodes
        .iter()
        .enumerate()
        .filter(|(i, n)| {
            (n.kind == "method" || n.kind == "property")
                && ["id", "name", "foo", "bar", "alpha", "beta"].contains(&n.name.as_str())
                && !declared.contains(i)
        })
        .map(|(_, n)| n)
        .collect();
    assert!(leaked.is_empty(), "{leaked:?}");
}

// Exported Variable Extraction: should extract exported const with call
// expression / object literal / array literal / Zod schema / XState machine
#[test]
fn ts_exported_consts() {
    for (file, code, name) in [
        (
            "store.ts",
            "\nexport const useUIStore = create<UIState>((set) => ({\n  isOpen: false,\n  toggle: () => set((s) => ({ isOpen: !s.isOpen })),\n}));\n",
            "useUIStore",
        ),
        (
            "config.ts",
            "\nexport const config = {\n  apiUrl: 'https://api.example.com',\n  timeout: 5000,\n};\n",
            "config",
        ),
        (
            "constants.ts",
            "\nexport const SCREEN_NAMES = ['home', 'settings', 'profile'] as const;\n",
            "SCREEN_NAMES",
        ),
        (
            "schemas.ts",
            "\nexport const userSchema = z.object({\n  id: z.string(),\n  name: z.string(),\n  email: z.string().email(),\n});\n",
            "userSchema",
        ),
        (
            "machine.ts",
            "\nexport const authMachine = createMachine({\n  id: \"auth\",\n  initial: \"idle\",\n  states: {\n    idle: {},\n    authenticated: {},\n  },\n});\n",
            "authMachine",
        ),
    ] {
        let x = ex(file, code);
        assert!(node(&x, "constant", name).is_exported, "{name}");
    }
}

// Exported Variable Extraction: should extract exported const with primitive
// value
#[test]
fn ts_exported_primitive_consts() {
    let x = ex(
        "constants.ts",
        r#"
export const MAX_RETRIES = 3;
export const API_VERSION = "v2";
"#,
    );
    assert_eq!(
        sorted(names(&x, "constant")),
        ["API_VERSION", "MAX_RETRIES"]
    );
}

// Exported Variable Extraction: should NOT duplicate arrow functions as both
// function and variable
#[test]
fn ts_arrow_not_duplicated_as_variable() {
    let x = ex(
        "hooks.ts",
        r#"
export const useAuth = () => {
  return useContext(AuthContext);
};
"#,
    );
    assert_eq!(
        x.nodes
            .iter()
            .filter(|n| n.kind == "function" && n.name == "useAuth")
            .count(),
        1
    );
    assert!(find(&x, "variable", "useAuth").is_none());
}

// Exported Variable Extraction: should extract non-exported const as
// non-exported variable
#[test]
fn ts_non_exported_const() {
    let x = ex(
        "internal.ts",
        r#"
const internalConfig = {
  debug: true,
};
"#,
    );
    let vars: Vec<&Node> = x
        .nodes
        .iter()
        .filter(|n| (n.kind == "variable" || n.kind == "constant") && n.name == "internalConfig")
        .collect();
    assert_eq!(vars.len(), 1);
    assert!(!vars[0].is_exported);
}

// Exported Variable Extraction: should extract calls from a top-level
// variable initializer (issue #425)
#[test]
fn ts_top_level_initializer_calls() {
    let x = ex(
        "app.ts",
        r#"
import { getTokenMp } from './api/upload';

const token = getTokenMp();
"#,
    );
    assert!(has_ref(&x, "calls", "getTokenMp"));
}

// Exported Variable Extraction > initializer walk is scoped to the declared
// symbol (#693 for TS/JS): a plain call initializer names the CONSTANT as
// caller, not the file; a non-exported object literal contributes calls
#[test]
fn ts_initializer_walk_scoped_to_symbol() {
    let x = ex(
        "app.ts",
        r#"
const eager = load();
const obj = { handler: () => target(), plain: target() };
const list = [() => target()];
export const exported = { handler: () => target() };
"#,
    );
    let callers_of = |name: &str| {
        sorted(
            refs(&x, "calls")
                .iter()
                .filter(|r| r.name == name)
                .map(|r| {
                    let n = owner(&x, r);
                    format!("{}:{}", n.kind, n.qualified_name)
                })
                .collect(),
        )
    };
    assert_eq!(callers_of("load"), ["constant:eager"]);
    assert_eq!(
        callers_of("target"),
        [
            "constant:list",
            "constant:obj",
            "function:exported::handler",
            "function:obj::handler",
        ]
    );
}

// Exported Variable Extraction: records the calls of a module-scope
// destructuring declaration (%s, #2340)
#[test]
fn module_scope_destructuring_calls() {
    let code = [
        "const { a } = useFoo(1)",
        "let [b, c] = pair()",
        "var { d: { e } } = nested()",
        "const { f = fallback() } = withDefault()",
        "const [g = other(), ...rest] = list()",
        "export const { h } = exported()",
        "const { i } = await load(() => inArrow())",
        "export function probe() { const { j } = inner(); return j }",
        "",
    ]
    .join("\n");
    for ext in ["ts", "tsx", "js", "jsx"] {
        let x = ex(&format!("app.{ext}"), &code);
        let calls: Vec<String> = refs(&x, "calls")
            .iter()
            .map(|r| format!("{}:{}@{}", owner(&x, r).kind, r.name, r.line))
            .collect();
        assert_eq!(
            sorted(calls),
            [
                "file:exported@6",
                "file:fallback@4",
                "file:inArrow@7",
                "file:list@5",
                "file:load@7",
                "file:nested@3",
                "file:other@5",
                "file:pair@2",
                "file:useFoo@1",
                "file:withDefault@4",
                "function:inner@8",
            ],
            "{ext}"
        );
        assert!(
            !x.nodes
                .iter()
                .any(|n| n.kind == "constant" || n.kind == "variable")
        );
    }
}

// kernel TS/JS parity: walks a module-scope destructuring declaration like a
// body does: %s (#2340)
#[test]
fn module_scope_destructuring_parity_fixture() {
    for (ext, typed) in [("ts", true), ("tsx", true), ("js", false), ("jsx", false)] {
        let src = format!(
            r#"
import {{ handler }} from './h';
const {{ a }} = useFoo(1);
let [b, c] = pair();
var {{ d: {{ e }} }} = nested();
const {{ f = fallback() }} = withDefault(handler);
const [g = other(), ...rest] = list(() => inArrow(handler));
export const {{ h }} = exported(new Store());
export const {{ useGetUserQuery }} = api;
{}
function probe() {{ const {{ j }} = inner(); return j; }}
export function second() {{ return probe(); }}
"#,
            if typed {
                "const { k }: Shape = make();"
            } else {
                ""
            }
        );
        let x = ex(&format!("fixture.{ext}"), &src);
        let calls = ref_names(&x, "calls");
        let mut want = vec![
            "useFoo",
            "pair",
            "nested",
            "fallback",
            "withDefault",
            "other",
            "list",
            "inArrow",
            "exported",
            "inner",
        ];
        if typed {
            want.push("make");
        }
        for name in want {
            assert!(
                calls.iter().any(|c| c == name),
                "{ext}: {name} in {calls:?}"
            );
        }
    }
}

// File Node Extraction: should create a file-kind node for each parsed file
#[test]
fn ts_file_node() {
    let x = ex(
        "greeter.ts",
        r#"
export function greet(name: string): string {
  return "Hello " + name;
}
"#,
    );
    let file = of_kind(&x, "file").next().expect("file");
    assert_eq!(file.name, "greeter.ts");
    assert_eq!(file.id, "file:greeter.ts");
    assert_eq!(detect_language("greeter.ts", ""), Some("typescript"));
    assert_eq!(file.start_line, 1);
}

// File Node Extraction: should create containment edges from file node to
// top-level declarations
#[test]
fn ts_file_contains_top_level_declarations() {
    let x = ex(
        "fns.ts",
        r#"
export function foo() {}
export function bar() {}
"#,
    );
    let file = idx(&x, of_kind(&x, "file").next().expect("file"));
    assert!(
        x.edges
            .iter()
            .filter(|e| e.source == file && e.kind == "contains")
            .count()
            >= 2
    );
}

// Import Extraction > TypeScript/JavaScript imports: default, named,
// namespace, side-effect, mixed, type, aliased named and relative path
// imports
#[test]
fn ts_import_forms() {
    let i = first_import("app.tsx", "import React from 'react';");
    assert_eq!(i.name, "react");
    assert_eq!(sig(&i), "import React from 'react';");

    let i = first_import(
        "icons.tsx",
        "import { Bug, Database } from '@phosphor-icons/react';",
    );
    assert_eq!(i.name, "@phosphor-icons/react");
    assert!(sig(&i).contains("Bug") && sig(&i).contains("Database"));

    let i = first_import(
        "icons.tsx",
        "import * as Icons from '@phosphor-icons/react';",
    );
    assert_eq!(i.name, "@phosphor-icons/react");
    assert!(sig(&i).contains("* as Icons"));

    assert_eq!(
        first_import("app.tsx", "import './styles.css';").name,
        "./styles.css"
    );

    let i = first_import(
        "app.tsx",
        "import React, { useState, useEffect } from 'react';",
    );
    assert_eq!(i.name, "react");
    for part in ["React", "useState", "useEffect"] {
        assert!(sig(&i).contains(part));
    }

    let i = first_import("types.ts", "import type { FC, ReactNode } from 'react';");
    assert_eq!(i.name, "react");
    assert!(sig(&i).contains("type") && sig(&i).contains("FC"));

    let i = first_import(
        "hooks.ts",
        "import { useState as useStateAlias } from 'react';",
    );
    assert_eq!(i.name, "react");
    assert!(sig(&i).contains("useState") && sig(&i).contains("useStateAlias"));

    let i = first_import(
        "components/Button.tsx",
        "import { helper } from '../utils/helper';",
    );
    assert_eq!(i.name, "../utils/helper");
    assert!(sig(&i).contains("helper"));
}

// Import Extraction > TypeScript/JavaScript imports: should extract multiple
// import statements
#[test]
fn ts_multiple_imports() {
    let x = ex(
        "app.tsx",
        r#"
import React from 'react';
import { Button } from './components';
import './styles.css';
"#,
    );
    assert_eq!(
        sorted(names(&x, "import")),
        ["./components", "./styles.css", "react"]
    );
}

// Instantiates + Decorates edge extraction: emits an instantiates ref for
// `new Foo()`; strips type-argument suffix from generic constructors; keeps
// trailing identifier from qualified `new ns.Foo()`
#[test]
fn ts_instantiates_refs() {
    let x = ex(
        "app.ts",
        "\nclass Foo {}\nfunction bootstrap() { return new Foo(); }\n",
    );
    assert!(has_ref(&x, "instantiates", "Foo"));

    let x = ex(
        "app.ts",
        "\nclass Container<T> { constructor(_: T) {} }\nfunction go() { return new Container<string>('x'); }\n",
    );
    assert_eq!(
        ref_names(&x, "instantiates").first().map(String::as_str),
        Some("Container")
    );

    let x = ex(
        "app.ts",
        "\nconst ns = { Foo: class {} };\nfunction go() { return new ns.Foo(); }\n",
    );
    assert_eq!(
        ref_names(&x, "instantiates").first().map(String::as_str),
        Some("Foo")
    );
}

// Instantiates + Decorates edge extraction: emits a decorates ref for `@Foo
// class X {}`
#[test]
fn ts_class_decorates_ref() {
    let x = ex(
        "app.ts",
        r#"
function Foo(_arg: string) { return (cls: any) => cls; }
@Foo('x')
class X {}
"#,
    );
    assert!(has_ref(&x, "decorates", "Foo"));
}

// Instantiates + Decorates edge extraction: does NOT attribute a prior
// class's decorator to the next class
#[test]
fn ts_decorator_not_attributed_to_next_class() {
    let x = ex(
        "app.ts",
        r#"
function A(cls: any) { return cls; }
function B(cls: any) { return cls; }
@A
class Foo {}
@B
class Bar {}
"#,
    );
    let from_bar: Vec<&Ref> = refs(&x, "decorates")
        .into_iter()
        .filter(|r| owner(&x, r).name == "Bar")
        .collect();
    assert_eq!(from_bar.len(), 1);
    assert_eq!(from_bar[0].name, "B");
}

// Instantiates + Decorates edge extraction: emits a decorates ref for `@Foo
// method() {}`
#[test]
fn ts_method_decorates_ref() {
    let x = ex(
        "app.ts",
        r#"
function Get(p: string) { return (t: any, k: string) => t; }
class Svc {
  @Get('/x') method() { return 1; }
}
"#,
    );
    let r = refs(&x, "decorates")
        .into_iter()
        .find(|r| r.name == "Get")
        .expect("decorates Get");
    assert_eq!(owner(&x, r).name, "method");
}

// Regression: issue-specific extraction fixes: indexes inner functions of an
// anonymous AMD/CommonJS module wrapper (#528)
#[test]
fn js_amd_wrapper_inner_functions() {
    let x = ex(
        "amd-module.js",
        r#"
define(['dep'], function (dep) {
  function innerHelper(x) { return x + 1; }
  function compute(y) { return innerHelper(y); }
  return { compute: compute };
});
"#,
    );
    let fns = names(&x, "function");
    assert!(fns.contains(&"innerHelper".to_string()));
    assert!(fns.contains(&"compute".to_string()));
}

// Regression: issue-specific extraction fixes: indexes new module
// extensions: .mts/.cts (TS) (#366, #556) -- the .mts half
#[test]
fn mts_parses_as_typescript() {
    let x = ex("mod.mts", "export function hello(): number { return 1; }");
    node(&x, "function", "hello");
}

// kernel TS/JS parity: named object literals own their members: %s (#2300)
#[test]
fn named_object_literals_own_members() {
    let source = [
        "/* é😀 */ const Api = { read() { helper(); }, close: () => helper(), gen: function* () { yield 1; }, 'quoted-key': function () {}, [dyn()]() { helper(); }, eager: helper(), alias: helper, helper };",
        "function helper() {}",
        "/** Saved. */",
        "export const Exported = { save() { helper(); } };",
        "(function () { const Local = { run() { helper(); } }, data = { x: 1 }; Local.run(); window.WS = { ws() { Local.run(); } }; })();",
        "App.utils = { pad(s) { return s; } };",
        "// The page.",
        "dw_page = { start() { helper(); } };",
        "function setup() { ns.late = { go() {} }; let h; h = { on() {} }; }",
        "module.exports = { cjs() {} };",
        "Foo.prototype = { proto() {} };",
        "self.handlers = { click() {} };",
        "consume({ ephemeral() {} });",
        "",
    ]
    .join("\n");
    for ext in ["ts", "tsx", "js", "jsx"] {
        for src in [source.clone(), crlf(&source)] {
            let x = ex(&format!("literal.{ext}"), &src);
            let qns: Vec<&str> = x.nodes.iter().map(|n| n.qualified_name.as_str()).collect();
            for qn in [
                "Api::read",
                "Api::close",
                "Api::gen",
                "Api::quoted-key",
                "Exported::save",
                "Local::run",
                "window.WS",
                "window.WS::ws",
                "App.utils::pad",
                "dw_page::start",
                "ns.late::go",
                "self.handlers::click",
            ] {
                assert!(qns.contains(&qn), "{ext}: {qn} in {qns:?}");
            }
            for name in ["cjs", "proto", "ephemeral", "data", "on"] {
                assert!(named(&x, name).is_none(), "{ext}: {name}");
            }
        }
    }
}

// kernel TS/JS parity: a minified bundle keeps its literals unowned: %s
// (#2300)
#[test]
fn minified_bundle_literals_unowned() {
    let line = "var a={b:function(){return c(1,2)},d:function(e){return e}};window.L={f:function(){return a.b()}};".repeat(40);
    for file in ["bundle.js", "vendor-min.js", "vendor.min.js"] {
        let x = ex(file, &format!("{line}\n{line}\n"));
        assert!(
            !of_kind(&x, "function").any(|n| n.name == "b" || n.name == "f"),
            "{file}"
        );
    }
}

// kernel TS/JS parity: a store exported by a later statement: %s / keeps its
// actions (LF, CRLF); an export clause names %s only whole
#[test]
fn store_exported_by_later_statement() {
    let has_inc = |x: &Extraction| find(x, "function", "inc").is_some();
    for ext in ["ts", "tsx", "js", "jsx"] {
        let file = format!("store.{ext}");
        for eol in ["\n", "\r\n"] {
            let src = [
                r#"import { create } from "zustand";"#,
                "const useStore = create((set) => ({ inc: () => set({}) }));",
                "export default useStore;",
                "",
            ]
            .join(eol);
            assert!(has_inc(&ex(&file, &src)), "{file} {eol:?}");
        }
        for (name, export_line, kept) in [
            ("items$", "export { items$ };", true),
            ("$items", "export { $items as default };", true),
            ("items", "export { items$ };", false),
            ("items", "export { $items };", false),
        ] {
            let src = [
                r#"import { create } from "zustand";"#.to_string(),
                format!("const {name} = create((set) => ({{ inc: () => set({{}}) }}));"),
                "const other = 1;".to_string(),
                export_line.to_string(),
                String::new(),
            ]
            .join("\n");
            assert_eq!(
                has_inc(&ex(&file, &src)),
                kept,
                "{file}: {name} / {export_line}"
            );
        }
    }
}

// kernel TS/JS parity: a store whose name has a `$`, exported by a later
// statement: %s -- keeps its actions; is not exported by a different name at
// a line end
#[test]
fn dollar_store_exported_by_later_statement() {
    let store = |name: &str, export_lines: &[&str]| {
        let mut lines = vec![
            r#"import { create } from "zustand";"#.to_string(),
            format!("const {name} = create((set) => ({{ inc: () => set({{}}) }}));"),
        ];
        lines.extend(export_lines.iter().map(|l| l.to_string()));
        lines.push(String::new());
        lines.join("\n")
    };
    for ext in ["ts", "tsx", "js", "jsx"] {
        let file = format!("store.{ext}");
        for (name, export_line) in [
            ("items$", "export default items$;"),
            ("$store", "export default $store;"),
            ("a$b", "export { a$b };"),
        ] {
            let x = ex(&file, &store(name, &[export_line]));
            assert!(find(&x, "function", "inc").is_some(), "{file}: {name}");
        }
        let x = ex(
            &file,
            &store("items$", &["const items = 1;", "export {", "  items", "};"]),
        );
        assert!(find(&x, "function", "inc").is_none(), "{file}");
    }
}

// kernel TS/JS parity: same-line accessors retain distinct identities after
// Unicode: %s (#1349)
#[test]
fn same_line_accessors_distinct_after_unicode() {
    let source = "class Point { /* é😀 */ get x() { return read(); } set x(v) { write(v); } }";
    let set_col = utf16_len(&source[..source.find("set x").unwrap()]);
    for ext in ["ts", "tsx", "js", "jsx"] {
        let x = ex(&format!("point.{ext}"), source);
        let xs: Vec<&Node> = x.nodes.iter().filter(|n| n.name == "x").collect();
        assert_eq!(xs.len(), 2, "{ext}");
        assert_eq!(xs[1].id, format!("{}:{set_col}", xs[0].id), "{ext}");
        let calls: Vec<(usize, String)> = refs(&x, "calls")
            .iter()
            .map(|r| (r.from, r.name.clone()))
            .collect();
        assert_eq!(
            calls,
            [
                (idx(&x, xs[0]), "read".to_string()),
                (idx(&x, xs[1]), "write".to_string())
            ],
            "{ext}"
        );
        assert_eq!(ex(&format!("point.{ext}"), source), x, "{ext}: repeatable");
    }
}

// kernel TS/JS parity: leaves nested identifier receivers unresolved and
// keeps argument calls: %s (#1566)
#[test]
fn nested_identifier_receivers_and_argument_calls() {
    let src = r#"
function readKey() { return 'answer'; }
function local() {
  const values = new Map();
  return values.get(readKey());
}
function nested(holder) {
  holder.values.get(readKey());
  holder.values?.get(readKey());
  holder['values'].get(readKey());
  holder.deep.values.get(readKey());
}
"#;
    for ext in ["ts", "tsx", "js", "jsx"] {
        let x = ex(&format!("fixture.{ext}"), src);
        assert_eq!(
            refs_from(&x, node(&x, "function", "nested"), "calls"),
            [
                "holder.values.get",
                "readKey",
                "holder.values.get",
                "readKey",
                "readKey",
                "holder.deep.values.get",
                "readKey",
            ],
            "{ext}"
        );
        assert!(x.refs.iter().any(|r| r.name == "values.get"), "{ext}");
    }
}

// kernel TS/JS parity: private field receivers: %s (#1987) -- preserves
// private fields and optional calls (LF, CRLF)
#[test]
fn private_field_receivers() {
    let source = r#"
class Mailer { send() {} }
class Vault {
  #mailer = new Mailer();
  #items = new Set();
  notify() { this.#mailer?.send(); }
  optional() { this.#mailer.send?.(); }
  put() { this.#items?.add('x'); }
}
"#;
    for ext in ["ts", "tsx", "js", "jsx"] {
        for src in [source.to_string(), crlf(source)] {
            let x = ex(&format!("vault.{ext}"), &src);
            assert_eq!(
                ref_names(&x, "calls"),
                ["this.#mailer.send", "this.#mailer.send", "this.#items.add"],
                "{ext}"
            );
        }
    }
}

// kernel TS/JS parity: peels transparent receivers and drops untyped
// expression receivers: %s
#[test]
fn transparent_receivers_peeled() {
    for (ext, typed) in [("ts", true), ("tsx", true), ("js", false), ("jsx", false)] {
        let src = format!(
            r#"
function list() {{ return []; }}
class Runner {{ go() {{ return 1; }} }}
async function exprReceivers(x, y) {{
  (await list()).map(g);
  (x).run();
  {}
  (a ?? b).map(g);
  arr[0].run();
  f().list.map(g);
  (() => 1).call(null);
  this.a.b.run();
  this.listeners[type].push(h);
  this[k].stop();
  this['svc'].start();
  {}
  super.stop();
  new Runner().go();
  (new ns.Widget(1)).draw();
  new (pick())().go();
  new Runner().a.run();
  new this.constructor().copy(x);
  new this().init();
  window.Api.start();
}}
"#,
            if typed {
                r#"x!.run(); (y as X).run(); (x satisfies X).stop(); getTarget("a")!.install(); if (x && y!.c.has(1)) {} if (x && this.e!.c.has(1)) {}"#
            } else {
                ""
            },
            if typed {
                "this.items![0].save(); (this.items[1] as X).save();"
            } else {
                ""
            },
        );
        let x = ex(&format!("fixture.{ext}"), &src);
        let mut want = vec!["list().map", "list", "x.run"];
        if typed {
            want.extend([
                "x.run",
                "y.run",
                "x.stop",
                "getTarget().install",
                "getTarget",
                "has",
            ]);
        }
        want.extend(["f", "run", "this.listeners[].push", "this.svc.start"]);
        if typed {
            want.extend(["this.items[].save", "this.items[].save"]);
        }
        want.extend([
            "stop",
            "new Runner().go",
            "new ns.Widget().draw",
            "pick",
            "copy",
            "init",
            "start",
        ]);
        assert_eq!(
            refs_from(&x, node(&x, "function", "exprReceivers"), "calls"),
            want,
            "{ext}"
        );
    }
}

// kernel TS/JS parity: typed styled tags are components, parsed as
// comparisons or not: %s
#[test]
fn typed_styled_tags_are_components() {
    let src = r#"
import styled, { css } from 'styled-components';
import { s } from './theme';
type Props = { align: 'start' | 'end' };
const Plain = styled.div`color: red;`;
export const Wrapper = styled.div<Props>`color: ${(p) => s(p.align)};`;
const CloseAction = styled.div<{ animation: Animation | null }>`top: 0;`;
const Content = styled(Plain)<Props>`padding: 4px;`;
const NudeButton = styled(Plain).attrs((props: Props) => ({ type: "button" }))<Props>``;
const Either = styled.div<Props | Other>`color: red;`;
const Nested = styled.div<Partial<Props>>`color: red;`;
const Mixin = css<Props>`color: red;`;
const Compared = styled.length < LIMIT > 2;
const lowerCase = styled.div<Props>`color: red;`;
"#;
    for ext in ["ts", "tsx"] {
        let x = ex(&format!("styles.{ext}"), src);
        let kinds = |name: &str| -> Vec<&str> {
            x.nodes
                .iter()
                .filter(|n| n.name == name)
                .map(|n| n.kind)
                .collect()
        };
        for name in [
            "Plain",
            "Wrapper",
            "CloseAction",
            "Content",
            "NudeButton",
            "Either",
            "Nested",
        ] {
            assert_eq!(kinds(name), ["component"], "{ext}: {name}");
        }
        for name in ["Mixin", "Compared", "lowerCase"] {
            assert_eq!(kinds(name), ["constant"], "{ext}: {name}");
        }
    }
}

// kernel TS/JS parity: edges the file-scope values a component reads or
// renders: %s
#[test]
fn component_value_reference_edges() {
    let src = r#"
import { lazy } from 'react';
import Loadable from './Loadable';
const LoginPage = Loadable(lazy(() => import('./pages/Login')));
const ROUTE_TABLE = [{ path: '/login', label: 'Login' }];
const THEME_TOKENS = { color: 'red', size: 12 };
export function AuthRoutes() { return <LoginPage />; }
export function Nav() {
  return <ul style={{ color: THEME_TOKENS.color }}>{ROUTE_TABLE.map((r) => <li key={r.path}>{r.label}</li>)}</ul>;
}
export const Box = () => <div data-size={THEME_TOKENS.size} />;
"#;
    for ext in ["tsx", "js", "jsx"] {
        let x = ex(&format!("routes.{ext}"), src);
        let value_refs: Vec<String> = x
            .edges
            .iter()
            .filter(|e| {
                e.kind == "references"
                    && e.metadata
                        .as_deref()
                        .and_then(|m| serde_json::from_str::<serde_json::Value>(m).ok())
                        .is_some_and(|m| m["valueRef"] == serde_json::Value::Bool(true))
            })
            .map(|e| format!("{} -> {}", x.nodes[e.source].name, x.nodes[e.target].name))
            .collect();
        assert_eq!(
            sorted(value_refs),
            [
                "AuthRoutes -> LoginPage",
                "Box -> THEME_TOKENS",
                "Nav -> ROUTE_TABLE",
                "Nav -> THEME_TOKENS",
            ],
            "{ext}"
        );
    }
}

// ---------------------------------------------------------------------------
// Python
// ---------------------------------------------------------------------------

// File Node Extraction: should create file nodes for Python files
#[test]
fn python_file_node() {
    let x = ex("main.py", "\ndef main():\n    pass\n");
    let file = of_kind(&x, "file").next().expect("file");
    assert_eq!(file.name, "main.py");
    assert_eq!(detect_language("main.py", ""), Some("python"));
}

// Python Extraction: should extract function definitions
#[test]
fn python_function_definitions() {
    let x = ex(
        "calc.py",
        r#"
def calculate_total(items: list, tax_rate: float) -> float:
    """Calculate total with tax."""
    subtotal = sum(item.price for item in items)
    return subtotal * (1 + tax_rate)
"#,
    );
    assert!(of_kind(&x, "file").next().is_some());
    assert_eq!(
        of_kind(&x, "function").next().expect("function").name,
        "calculate_total"
    );
}

// Python Extraction: should extract class definitions
#[test]
fn python_class_definitions() {
    let x = ex(
        "service.py",
        r#"
class UserService:
    """Service for managing users."""

    def __init__(self, db):
        self.db = db

    def get_user(self, user_id: str) -> User:
        return self.db.find_user(user_id)
"#,
    );
    assert_eq!(
        of_kind(&x, "class").next().expect("class").name,
        "UserService"
    );
}

// TypeScript Extraction: captures docstrings for decorated Python
// declarations, stripping `#` (#780)
#[test]
fn python_decorated_docstrings() {
    let code = [
        "# decorated function",
        "@app.route(\"/x\")",
        "def py_handler():",
        "    return 1",
        "",
        "",
        "# plain function control",
        "def py_plain():",
        "    return 1",
        "",
        "",
        "# decorated class",
        "@dataclass",
        "class PyModel:",
        "    pass",
        "",
    ]
    .join("\n");
    let x = ex("mod.py", &code);
    let doc = |name: &str| named(&x, name).and_then(|n| n.docstring.clone());
    assert_eq!(doc("py_handler").as_deref(), Some("decorated function"));
    assert_eq!(doc("py_plain").as_deref(), Some("plain function control"));
    assert_eq!(doc("PyModel").as_deref(), Some("decorated class"));
}

// Python Extraction: walks a module-level assignment initializer scoped to
// the name (#693 for Python)
#[test]
fn python_module_assignment_initializers() {
    let x = ex(
        "app.py",
        r#"
def target(): pass
def compute(): return 1

APP = compute()
handler = lambda: target()
MAPPING = {"a": compute()}
first, second = compute(), target()

class K:
    ATTR = compute()
"#,
    );
    let owners: Vec<String> = refs(&x, "calls")
        .iter()
        .map(|r| {
            let n = owner(&x, r);
            format!("{}<-{}:{}", r.name, n.kind, n.name)
        })
        .collect();
    assert_eq!(
        sorted(owners),
        [
            "compute<-class:K",
            "compute<-file:app.py",
            "compute<-variable:APP",
            "compute<-variable:MAPPING",
            "target<-file:app.py",
            "target<-variable:handler",
        ]
    );
}

// Import Extraction > Python imports: simple, from, multiple names, aliased,
// relative and wildcard imports
#[test]
fn python_import_forms() {
    assert_eq!(first_import("utils.py", "import json").name, "json");

    let i = first_import("utils.py", "from os import path");
    assert_eq!(i.name, "os");
    assert!(sig(&i).contains("path"));

    let i = first_import("types.py", "from typing import List, Dict, Optional");
    assert_eq!(i.name, "typing");
    assert!(sig(&i).contains("List") && sig(&i).contains("Dict"));

    let i = first_import("data.py", "import numpy as np");
    assert_eq!(i.name, "numpy");
    assert!(sig(&i).contains("as np"));

    let i = first_import("module.py", "from .utils import helper");
    assert_eq!(i.name, ".utils");
    assert!(sig(&i).contains("helper"));

    let i = first_import("types.py", "from typing import *");
    assert_eq!(i.name, "typing");
    assert!(sig(&i).contains('*'));
}

// Import Extraction > Python imports: should extract multiple import
// statements
#[test]
fn python_multiple_imports() {
    let x = ex("main.py", "\nimport os\nimport sys\n");
    assert_eq!(sorted(names(&x, "import")), ["os", "sys"]);
}

// kernel TS/JS parity: Python member values preserve receivers across
// callback, assignment and collection positions (#1820)
#[test]
fn python_member_values_keep_receivers() {
    let x = ex(
        "members.py",
        r#"
class Store:
    def fetch(self, ids):
        return ids
class Consumer:
    def wire(self, pool, obj):
        pool.submit(self.store.fetch, obj.fetch)
        cb = self.store.fetch
        table = [obj.fetch, Store.fetch, self.fetch, cls.fetch]
        keyword(callback=obj.fetch)
        obj.fetch([])
        pool.submit(factory().fetch, obj[0].fetch)
"#,
    );
    assert_eq!(
        sorted(ref_names(&x, "function_ref")),
        [
            "Store.fetch",
            "cls.fetch",
            "obj.fetch",
            "self.fetch",
            "self.store.fetch"
        ]
    );
    assert!(has_ref(&x, "calls", "obj.fetch"));
}

// ---------------------------------------------------------------------------
// Go
// ---------------------------------------------------------------------------

// Go Extraction: should extract function declarations
#[test]
fn go_function_declarations() {
    let x = ex(
        "main.go",
        r#"
package main

func ProcessOrder(order Order) (Receipt, error) {
    // Process the order
    return Receipt{}, nil
}
"#,
    );
    assert_eq!(
        of_kind(&x, "function").next().expect("function").name,
        "ProcessOrder"
    );
}

// Go Extraction: should extract method declarations
#[test]
fn go_method_declarations() {
    let x = ex(
        "service.go",
        r#"
package main

type Service struct {
    db *Database
}

func (s *Service) GetUser(id string) (*User, error) {
    return s.db.FindUser(id)
}
"#,
    );
    assert_eq!(
        of_kind(&x, "method").next().expect("method").name,
        "GetUser"
    );
}

// Regression: issue-specific extraction fixes: attaches Go methods on
// generic receivers to their type (#583)
#[test]
fn go_generic_receiver_methods() {
    let x = ex(
        "stack.go",
        r#"
package main

type Stack[T any] struct { items []T }

func (s *Stack[T]) Push(v T) { s.items = append(s.items, v) }
func (s Stack[T]) Len() int { return len(s.items) }
"#,
    );
    assert_eq!(node(&x, "method", "Push").qualified_name, "Stack::Push");
    assert_eq!(node(&x, "method", "Len").qualified_name, "Stack::Len");
}

// Import Extraction > Go imports: single, aliased, dot and blank imports
#[test]
fn go_import_forms() {
    assert_eq!(
        first_import("main.go", "\npackage main\n\nimport \"fmt\"\n").name,
        "fmt"
    );

    let i = first_import("main.go", "\npackage main\n\nimport f \"fmt\"\n");
    assert_eq!(i.name, "fmt");
    assert!(sig(&i).contains('f'));

    let i = first_import("main.go", "\npackage main\n\nimport . \"math\"\n");
    assert_eq!(i.name, "math");
    assert!(sig(&i).contains('.'));

    let i = first_import(
        "main.go",
        "\npackage main\n\nimport _ \"github.com/go-sql-driver/mysql\"\n",
    );
    assert_eq!(i.name, "github.com/go-sql-driver/mysql");
    assert!(sig(&i).contains('_'));
}

// Import Extraction > Go imports: should extract grouped imports
#[test]
fn go_grouped_imports() {
    let x = ex(
        "main.go",
        "\npackage main\n\nimport (\n\t\"fmt\"\n\t\"os\"\n\t\"encoding/json\"\n)\n",
    );
    assert_eq!(sorted(names(&x, "import")), ["encoding/json", "fmt", "os"]);
}

// kernel TS/JS parity: Go method values preserve receivers and exclude
// invocation receivers (#1820)
#[test]
fn go_method_values_keep_receivers() {
    let x = ex(
        "members.go",
        r#"package demo
 type Store struct{}
 func (s *Store) Fetch() {}
 func wire(c *Store, pool Pool) {
   Submit(c.Fetch)
   cb := c.Fetch
   table := []func(){c.Fetch, Store.Fetch}
   Submit(c.store.Fetch)
   go c.Fetch()
   Submit(factory().Fetch, items[0].Fetch)
 }
"#,
    );
    assert_eq!(
        sorted(ref_names(&x, "function_ref")),
        ["Store.Fetch", "c.Fetch", "c.store.Fetch"]
    );
    assert!(has_ref(&x, "calls", "c.Fetch"));
}

// kernel TS/JS parity: Go selector values stoplist the whole name, not the
// field (LF, CRLF)
#[test]
fn go_selector_values_stoplist_whole_name() {
    let source = "package demo\nimport \"example.com/raft\"\ntype Store struct{}\nfunc (s *Store) new() any { return nil }\nfunc wire(s *Store) {\n\tSubmit(raft.None, C.NULL, None, nil)\n\tpool := sync.Pool{New: s.new}\n\tSubmit(raft.\n\t\tNext, raft./* c */ Prev, raft.// c\n\t\tLast)\n\tSubmit(s.\n\t\tinner.Fetch)\n\t_ = pool\n}\n";
    for src in [source.to_string(), crlf(source)] {
        let x = ex("values.go", &src);
        assert_eq!(
            sorted(ref_names(&x, "function_ref")),
            [
                "C.NULL",
                "raft.Last",
                "raft.Next",
                "raft.None",
                "raft.Prev",
                "s.new"
            ]
        );
    }
}

// ---------------------------------------------------------------------------
// Rust
// ---------------------------------------------------------------------------

// Rust Extraction: should extract function declarations
#[test]
fn rust_function_declarations() {
    let x = ex(
        "lib.rs",
        r#"
pub fn process_data(input: &str) -> Result<Output, Error> {
    // Process data
    Ok(Output::new())
}
"#,
    );
    let f = of_kind(&x, "function").next().expect("function");
    assert_eq!(f.name, "process_data");
    assert_eq!(f.visibility, Some("public"));
}

// Rust Extraction: should extract struct declarations
#[test]
fn rust_struct_declarations() {
    let x = ex(
        "models.rs",
        r#"
pub struct User {
    pub id: String,
    pub name: String,
    email: String,
}
"#,
    );
    assert_eq!(of_kind(&x, "struct").next().expect("struct").name, "User");
}

// Rust Extraction: should extract unit and tuple structs, not just brace
// structs
#[test]
fn rust_unit_and_tuple_structs() {
    let x = ex(
        "shapes.rs",
        r#"
pub struct Unit;
pub struct Tuple(pub u32);
pub struct Brace { pub x: u32 }
"#,
    );
    assert_eq!(sorted(names(&x, "struct")), ["Brace", "Tuple", "Unit"]);
}

// Rust Extraction: should link impl Trait for a unit struct
#[test]
fn rust_impl_trait_for_unit_struct() {
    let x = ex(
        "greet.rs",
        r#"
pub struct Unit;
pub trait Greet { fn hi(&self) -> String; }
impl Greet for Unit { fn hi(&self) -> String { "unit".into() } }
"#,
    );
    node(&x, "struct", "Unit");
    node(&x, "trait", "Greet");
}

// Rust Extraction: should extract trait declarations
#[test]
fn rust_trait_declarations() {
    let x = ex(
        "traits.rs",
        r#"
pub trait Repository {
    fn find(&self, id: &str) -> Option<Entity>;
    fn save(&mut self, entity: Entity) -> Result<(), Error>;
}
"#,
    );
    assert_eq!(
        of_kind(&x, "trait").next().expect("trait").name,
        "Repository"
    );
}

// Rust Extraction: should extract impl Trait for Type as implements edges
#[test]
fn rust_impl_trait_implements_ref() {
    let x = ex(
        "cache.rs",
        r#"
pub struct MyCache {}

pub trait Cache {
    fn get(&self, key: &str) -> Option<String>;
}

impl Cache for MyCache {
    fn get(&self, key: &str) -> Option<String> {
        None
    }
}
"#,
    );
    let r = refs(&x, "implements")
        .into_iter()
        .find(|r| r.name == "Cache")
        .expect("implements Cache");
    assert_eq!(r.from, idx(&x, node(&x, "struct", "MyCache")));
}

// Rust Extraction: qualifies methods of a generic or lifetime impl by the
// implementing type, not the trait (#1588)
#[test]
fn rust_generic_impl_methods_qualified_by_type() {
    let x = ex(
        "src.rs",
        r#"
pub trait Source {
    fn read(&mut self) -> usize;
}

pub struct FileSource { pub n: usize }
impl Source for FileSource {
    fn read(&mut self) -> usize { self.n }
}

pub struct BufSource<T> { pub inner: T }
impl<T> Source for BufSource<T> {
    fn read(&mut self) -> usize { 0 }
}

pub struct Parents<'a> { cur: &'a u32 }
impl<'a> Iterator for Parents<'a> {
    type Item = u32;
    fn next(&mut self) -> Option<u32> { None }
}

pub struct Wrapper { pub n: usize }
impl Source for &Wrapper {
    fn read(&mut self) -> usize { 1 }
}

pub mod m { pub struct Scoped { pub n: usize } }
impl Source for m::Scoped {
    fn read(&mut self) -> usize { 2 }
}

pub struct Own { pub n: usize }
impl From<u32> for Own {
    fn from(n: u32) -> Self { Own { n: n as usize } }
}
"#,
    );
    let qns = sorted(
        of_kind(&x, "method")
            .map(|n| n.qualified_name.clone())
            .collect(),
    );
    assert_eq!(
        qns,
        [
            "BufSource::read",
            "FileSource::read",
            "Own::from",
            "Parents::next",
            "Scoped::read",
            "Source::read",
            "Wrapper::read",
        ]
    );
    let trait_read: Vec<&Node> = x
        .nodes
        .iter()
        .filter(|n| n.qualified_name == "Source::read")
        .collect();
    assert_eq!(trait_read.len(), 1);
    assert_eq!(trait_read[0].start_line, 3);
    let implements_from = |ty: &str| refs_from(&x, node(&x, "struct", ty), "implements");
    assert_eq!(implements_from("FileSource"), ["Source"]);
    assert_eq!(implements_from("BufSource"), ["Source"]);
    assert_eq!(implements_from("Parents"), ["Iterator"]);
    assert_eq!(implements_from("Wrapper"), ["Source"]);
    assert_eq!(implements_from("Scoped"), ["Source"]);
    assert_eq!(implements_from("Own"), ["From<u32>"]);
    let buf = node(&x, "struct", "BufSource");
    let buf_read = x
        .nodes
        .iter()
        .find(|n| n.qualified_name == "BufSource::read")
        .unwrap();
    assert!(contains(&x, buf, buf_read));
}

// Rust Extraction: keeps the owner shape for `self.<method>()` and
// `self.<field>.<method>()`, a longer receiver chain whole, and collapses a
// parenthesized receiver (#1585, #1861, #2836)
#[test]
fn rust_self_receiver_shapes() {
    let x = ex(
        "outer.rs",
        r#"
pub struct Outer { pub inner: Inner, pub deep: Deep }
impl Outer {
    pub fn run(&mut self) {
        self.inner.run();
        self.deep.inner.run();
        self.make().run();
        (self.inner).run();
        self.run();
        let local = Inner { n: 0 };
        local.run();
    }
}
"#,
    );
    let calls = ref_names(&x, "calls");
    assert_eq!(
        sorted(
            calls
                .iter()
                .filter(|c| c.starts_with("self."))
                .cloned()
                .collect()
        ),
        [
            "self.deep.inner.run",
            "self.inner.run",
            "self.make",
            "self.make().run",
            "self.run",
        ]
    );
    assert!(calls.contains(&"local.run".to_string()));
    assert_eq!(calls.iter().filter(|c| *c == "run").count(), 1);
    let outer_run = x
        .nodes
        .iter()
        .find(|n| n.qualified_name == "Outer::run")
        .expect("Outer::run");
    let field_ref = x.refs.iter().find(|r| r.name == "self.inner.run").unwrap();
    assert_eq!(field_ref.from, idx(&x, outer_run));
    assert_eq!(field_ref.line, 5);
}

// Rust Extraction: gives no receiver to an impl whose target names no single
// type
#[test]
fn rust_impl_without_single_type_target() {
    let x = ex(
        "src.rs",
        r#"
pub trait Base { fn id(&self) -> u32; }
impl Base for (u32, u32) {
    fn id(&self) -> u32 { 0 }
}
impl Base for dyn Base {
    fn id(&self) -> u32 { 1 }
}
"#,
    );
    let ids: Vec<&Node> = x.nodes.iter().filter(|n| n.name == "id").collect();
    assert_eq!(
        sorted(ids.iter().map(|n| n.qualified_name.clone()).collect()),
        ["Base::id", "id", "id"]
    );
    assert_eq!(ids.iter().filter(|n| n.kind == "function").count(), 2);
    assert!(refs(&x, "implements").is_empty());
}

// Rust Extraction: should extract trait supertraits as extends references
#[test]
fn rust_supertraits_are_extends_refs() {
    let x = ex(
        "error.rs",
        r#"
pub trait Display {}

pub trait Error: Display {
    fn description(&self) -> &str;
}
"#,
    );
    let r = refs(&x, "extends")
        .into_iter()
        .find(|r| r.name == "Display")
        .expect("extends Display");
    assert_eq!(r.from, idx(&x, node(&x, "trait", "Error")));
}

// Rust Extraction: should not create implements edges for plain impl blocks
#[test]
fn rust_plain_impl_has_no_implements() {
    let x = ex(
        "counter.rs",
        r#"
pub struct Counter {
    count: u32,
}

impl Counter {
    pub fn new() -> Counter {
        Counter { count: 0 }
    }
    pub fn increment(&mut self) {
        self.count += 1;
    }
}
"#,
    );
    assert!(refs(&x, "implements").is_empty());
}

// Rust Extraction: walks a const/static initializer scoped to the declared
// symbol (#693 for Rust)
#[test]
fn rust_const_static_initializers() {
    let x = ex(
        "lib.rs",
        r#"
const LEN: usize = compute_len();
static REGISTRY: Lazy<Cfg> = Lazy::new(|| build_cfg());
"#,
    );
    let owner_of = |name: &str| {
        refs(&x, "calls").iter().find(|r| r.name == name).map(|r| {
            let n = owner(&x, r);
            format!("{}:{}", n.kind, n.name)
        })
    };
    assert_eq!(owner_of("compute_len").as_deref(), Some("variable:LEN"));
    assert_eq!(owner_of("build_cfg").as_deref(), Some("variable:REGISTRY"));
}

// Rust Extraction: should extract union declarations and their impl edges
#[test]
fn rust_union_declarations() {
    let x = ex(
        "reg.rs",
        r#"
pub union Reg {
    pub raw: u32,
    pub halves: [u16; 2],
}

pub trait Describe {
    fn describe(&self) -> u32;
}

impl Describe for Reg {
    fn describe(&self) -> u32 {
        unsafe { self.raw }
    }
}
"#,
    );
    let reg = named(&x, "Reg").expect("Reg");
    assert_eq!(reg.kind, "union");
    let r = refs(&x, "implements")
        .into_iter()
        .find(|r| r.name == "Describe")
        .expect("implements Describe");
    assert_eq!(r.from, idx(&x, reg));
    let method = of_kind(&x, "method")
        .find(|n| n.qualified_name.contains("Reg"))
        .expect("Reg method");
    assert!(contains(&x, reg, method));
}

// Import Extraction > Rust imports: simple use, scoped use list, crate,
// super and external crate imports
#[test]
fn rust_import_forms() {
    let i = first_import("main.rs", "use std::io;");
    assert_eq!(i.name, "std");
    assert_eq!(sig(&i), "use std::io;");

    let i = first_import("main.rs", "use std::{ffi::OsStr, io, path::Path};");
    assert_eq!(i.name, "std");
    assert!(sig(&i).contains("ffi::OsStr") && sig(&i).contains("path::Path"));

    assert_eq!(
        first_import("lib.rs", "use crate::error::Error;").name,
        "crate"
    );
    assert_eq!(first_import("submod.rs", "use super::utils;").name, "super");

    let i = first_import("types.rs", "use serde::{Serialize, Deserialize};");
    assert_eq!(i.name, "serde");
    assert!(sig(&i).contains("Serialize") && sig(&i).contains("Deserialize"));
}

// kernel Rust parity: variant paths reference their receiver (#2328)
#[test]
fn rust_variant_paths_reference_receiver() {
    let source = r#"
use crate::mode::{self, Mode};
const DEFAULT: Mode = Mode::A;
impl Mode {
    fn flip(&self) -> u8 {
        let _s = "ü"; let _x = Self::B;
        match self { Self::A => 1, Mode::C(x) => *x, mode::Mode::D { x } => *x, _ => 0 }
    }
}
trait Tr { fn t(&self) { let _ = Self::K; } }
fn uses() {
    use crate::mode::Mode::A;
    let _a = crate::mode::Mode::A; let _b = Mode::C(1); let _c = Mode::D { x: 2 };
    let _d = Mode::new(); let _e = util::take(3); let _f = u8::MAX; let _g = Limits::MAX;
    let _h = xs.map(Mode::C); let _i = Option::<u8>::None; let _j = Mode::A.flip();
    if let Mode::C(x) | Mode::B = m {}
    match o { Option::Some(_) => std::cmp::Ordering::Less, _ => cmp::Ordering::Greater };
}
"#;
    let x = ex("variants.rs", source);
    let lines: Vec<&str> = source.split('\n').collect();
    // `/^\w+\s*::/` on the line from the ref's UTF-16 column.
    let at_path = |r: &Ref| {
        let line: Vec<u16> = lines[r.line as usize - 1].encode_utf16().collect();
        let rest = String::from_utf16_lossy(&line[r.column as usize..]);
        let word = rest
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
            .count();
        word > 0 && rest[word..].trim_start().starts_with("::")
    };
    let paths: Vec<String> = refs(&x, "references")
        .into_iter()
        .filter(|r| at_path(r))
        .map(|r| format!("{}:{}", r.name, r.line))
        .collect();
    assert_eq!(
        sorted(paths),
        [
            "Limits:14",
            "Mode:13",
            "Mode:15",
            "Mode:15",
            "Mode:16",
            "Mode:16",
            "Mode:3",
            "Mode:6",
            "Mode:7",
            "Mode:7",
            "Mode:7",
            "Option:17",
            "Ordering:17",
            "Ordering:17",
        ]
    );
}

// kernel Rust parity: a call keeps its receiver chain of fields and method
// calls (#2836)
#[test]
fn rust_call_keeps_receiver_chain() {
    let links: Vec<String> = (0..17).map(|i| format!("l{i}()")).collect();
    let source = format!(
        r#"
pub struct Svc {{ services: Services, store: Box<dyn Store>, pair: (u8, Config) }}
impl Svc {{
    fn chains(&self, vis: &Vis) {{
        self.services.store.put("a"); let _ = self.services.store.put("ü");
        self
            .services // rustfmt
            .store
            .put("c");
        self.store().put("m"); self.store.put("d"); self.put("e"); vis.put(1);
        vis.map.get("k").get(0); self.0.len(); self.pair.1.port();
        vis.get::<u8>(1).put::<u16>(2); vis.a.b().c.d();
        Foo::new().a().b(); (vis).a.b(); self.a?.b(); self.a.b().await.c(); vis[0].a.b();
        r#vis.a.b(); vis.{}.m();
    }}
}}
"#,
        links.join(".")
    );
    let x = ex("chains.rs", &source);
    let calls = ref_names(&x, "calls");
    for name in [
        "self.services.store.put",
        "self.store().put",
        "self.store.put",
        "self.put",
        "vis.put",
        "vis.map.get",
        "vis.map.get().get",
        "self.0.len",
        "self.pair.1.port",
        "vis.get",
        "vis.get().put",
        "vis.a.b().c.d",
        "Foo::new().a",
        "b",
        "c",
        "m",
    ] {
        assert!(calls.iter().any(|c| c == name), "{name} in {calls:?}");
    }
}

// ---------------------------------------------------------------------------
// Java
// ---------------------------------------------------------------------------

// Java Extraction: should extract class declarations
#[test]
fn java_class_declarations() {
    let x = ex(
        "UserService.java",
        r#"
public class UserService {
    private final UserRepository repository;

    public UserService(UserRepository repository) {
        this.repository = repository;
    }

    public User getUser(String id) {
        return repository.findById(id);
    }
}
"#,
    );
    let class = of_kind(&x, "class").next().expect("class");
    assert_eq!(class.name, "UserService");
    assert_eq!(class.visibility, Some("public"));
}

// Java Extraction: should extract method declarations
#[test]
fn java_method_declarations() {
    let x = ex(
        "Calculator.java",
        r#"
public class Calculator {
    public static int add(int a, int b) {
        return a + b;
    }
}
"#,
    );
    assert!(node(&x, "method", "add").is_static);
}

// Java Extraction: wraps top-level declarations in a namespace from
// package_declaration
#[test]
fn java_package_namespace() {
    let x = ex(
        "Bar.java",
        r#"
package com.example.foo;

public class Bar {
    public String greet() { return "hi"; }
}
"#,
    );
    assert_eq!(
        of_kind(&x, "namespace").next().expect("namespace").name,
        "com.example.foo"
    );
    assert_eq!(
        node(&x, "class", "Bar").qualified_name,
        "com.example.foo::Bar"
    );
    assert_eq!(
        node(&x, "method", "greet").qualified_name,
        "com.example.foo::Bar::greet"
    );
}

// Java Extraction: does not wrap when no package is declared
#[test]
fn java_no_package_no_namespace() {
    let x = ex(
        "Bar.java",
        r#"
public class Bar {
    public String greet() { return "hi"; }
}
"#,
    );
    assert!(of_kind(&x, "namespace").next().is_none());
    assert_eq!(node(&x, "class", "Bar").qualified_name, "Bar");
}

// Java Extraction: extracts anonymous-class overrides from `new T() { ... }`
#[test]
fn java_anonymous_class_overrides() {
    let x = ex(
        "Factory.java",
        r#"
package com.example;

abstract class Base {
  abstract int compute(int x);
}

public class Factory {
  public Base make() {
    return new Base() {
      @Override
      int compute(int x) { return x + 1; }
    };
  }
}
"#,
    );
    let anon = of_kind(&x, "class")
        .find(|n| n.name.contains("Base$anon@"))
        .expect("anonymous Base subclass");
    let compute = of_kind(&x, "method")
        .find(|n| n.name == "compute" && n.qualified_name.contains("$anon@"))
        .expect("override on the anon class");
    assert!(
        compute
            .qualified_name
            .contains("Factory::make::<Base$anon@")
    );
    assert!(compute.qualified_name.ends_with("::compute"));
    let anon_i = idx(&x, anon);
    assert!(
        x.refs
            .iter()
            .any(|r| r.kind == "extends" && r.name == "Base" && r.from == anon_i)
    );
    assert!(has_ref(&x, "instantiates", "Base"));
}

// Java Extraction: extracts anonymous-class overrides inside a lambda body
#[test]
fn java_anonymous_class_in_lambda() {
    let x = ex(
        "Splitter.java",
        r#"
package com.example;

interface Strategy {
  java.util.Iterator<String> iterator(String s);
}

abstract class BaseIter implements java.util.Iterator<String> {
  abstract int separatorStart(int start);
}

public class Splitter {
  private final Strategy strategy;
  public Splitter(Strategy s) { this.strategy = s; }

  public static Splitter on(char c) {
    return new Splitter((seq) ->
        new BaseIter() {
          @Override
          int separatorStart(int start) { return start + 1; }
          @Override public boolean hasNext() { return false; }
          @Override public String next() { return null; }
        });
  }
}
"#,
    );
    assert!(of_kind(&x, "class").any(|n| n.name.contains("BaseIter$anon@")));
    assert!(
        of_kind(&x, "method")
            .any(|n| n.name == "separatorStart" && n.qualified_name.contains("$anon@"))
    );
}

// Java Extraction: walks a field initializer scoped to the field (#693 for
// Java)
#[test]
fn java_field_initializers() {
    let x = ex(
        "T.java",
        r#"
package p;
class T {
    private final Runnable fieldLambda = () -> target();
    private final Runnable anonClass = new Runnable() {
        public void run() { target(); }
    };
    private final int eager = compute();
    void directCall() { target(); }
    private void target() {}
    private static int compute() { return 1; }
}
"#,
    );
    let callers_of = |name: &str| {
        sorted(
            refs(&x, "calls")
                .iter()
                .filter(|r| r.name == name)
                .map(|r| owner(&x, r).name.clone())
                .collect(),
        )
    };
    assert_eq!(callers_of("target"), ["directCall", "fieldLambda", "run"]);
    assert_eq!(callers_of("compute"), ["eager"]);
}

// Import Extraction > Java imports: simple, static, wildcard and nested
// class imports
#[test]
fn java_import_forms() {
    let i = first_import("Main.java", "import java.util.List;");
    assert_eq!(i.name, "java.util.List");
    assert_eq!(sig(&i), "import java.util.List;");

    let i = first_import(
        "Utils.java",
        "import static java.util.Collections.emptyList;",
    );
    assert_eq!(i.name, "java.util.Collections.emptyList");
    assert!(sig(&i).contains("static"));

    let i = first_import("App.java", "import java.util.*;");
    assert_eq!(i.name, "java.util");
    assert!(sig(&i).contains(".*"));

    assert_eq!(
        first_import("MapUtil.java", "import java.util.Map.Entry;").name,
        "java.util.Map.Entry"
    );
}

// Import Extraction > Java imports: should extract multiple imports
#[test]
fn java_multiple_imports() {
    let x = ex(
        "Service.java",
        "\nimport java.util.List;\nimport java.util.Map;\nimport java.io.IOException;\n",
    );
    assert_eq!(
        sorted(names(&x, "import")),
        ["java.io.IOException", "java.util.List", "java.util.Map"]
    );
}

// ---------------------------------------------------------------------------
// Ruby
// ---------------------------------------------------------------------------

// Import Extraction > Ruby imports: require, require with path,
// require_relative; should not extract non-require calls
#[test]
fn ruby_require_forms() {
    let i = first_import("app.rb", "require 'json'");
    assert_eq!(i.name, "json");
    assert_eq!(sig(&i), "require 'json'");

    assert_eq!(
        first_import("config.rb", "require 'active_support/core_ext/string'").name,
        "active_support/core_ext/string"
    );

    let i = first_import("test/my_test.rb", "require_relative '../test_helper'");
    assert_eq!(i.name, "../test_helper");
    assert!(sig(&i).contains("require_relative"));

    assert!(
        of_kind(&ex("app.rb", "puts 'hello'"), "import")
            .next()
            .is_none()
    );
}

// Import Extraction > Ruby imports: should extract multiple requires
#[test]
fn ruby_multiple_requires() {
    let x = ex(
        "lib.rb",
        "\nrequire 'json'\nrequire 'yaml'\nrequire_relative 'helper'\n",
    );
    assert_eq!(sorted(names(&x, "import")), ["helper", "json", "yaml"]);
}

// Import Extraction > Ruby modules: should extract module as module node
// with containment
#[test]
fn ruby_module_with_containment() {
    let x = ex(
        "concerns/cached_counting.rb",
        r#"
module CachedCounting
  def self.disable
    @enabled = false
  end

  def perform_increment!(key, count)
    write_cache!(key, count)
  end
end
"#,
    );
    let module = node(&x, "module", "CachedCounting");
    assert_eq!(module.qualified_name, "CachedCounting");
    assert_eq!(
        node(&x, "method", "disable").qualified_name,
        "CachedCounting::disable"
    );
    assert_eq!(
        node(&x, "method", "perform_increment!").qualified_name,
        "CachedCounting::perform_increment!"
    );
    let m = idx(&x, module);
    assert!(
        x.edges
            .iter()
            .filter(|e| e.source == m && e.kind == "contains")
            .count()
            >= 2
    );
}

// Import Extraction > Ruby modules: should handle nested modules with
// classes
#[test]
fn ruby_nested_modules_with_classes() {
    let x = ex(
        "lib/auth.rb",
        r#"
module Discourse
  module Auth
    class AuthProvider
      def authenticate(params)
        validate(params)
      end
    end
  end
end
"#,
    );
    node(&x, "module", "Discourse");
    assert_eq!(node(&x, "module", "Auth").qualified_name, "Discourse::Auth");
    assert_eq!(
        node(&x, "class", "AuthProvider").qualified_name,
        "Discourse::Auth::AuthProvider"
    );
    assert_eq!(
        named(&x, "authenticate")
            .expect("authenticate")
            .qualified_name,
        "Discourse::Auth::AuthProvider::authenticate"
    );
}

// kernel Ruby parity: namespaced constant receivers
#[test]
fn ruby_namespaced_constant_receivers() {
    let src = [
        "class Callers",
        "  def a",
        "    Billing::ChargeCard.call(amount: 1)",
        "    ::Billing::ChargeCard.call",
        "    ::Foo.call",
        "    Users::Create&.call",
        "    A::B::C.run!",
        "    Config::DEFAULTS.fetch(:x)",
        "    Foo::Bar::baz",
        "    obj::Thing.go",
        "    self.class::LIMIT.times { }",
        "    Billing::ChargeCard.new(amount: 3)",
        "  end",
        "end",
        "",
    ]
    .join("\n");
    let x = ex("src/callers.rb", &src);
    let got: Vec<String> = x
        .refs
        .iter()
        .filter(|r| r.kind == "references" || r.kind == "instantiates")
        .map(|r| format!("{} {} {}:{}", r.kind, r.name, r.line, r.column))
        .collect();
    assert_eq!(
        sorted(got),
        [
            "instantiates ChargeCard 12:4",
            "references ::Billing::ChargeCard 4:4",
            "references ::Foo 5:4",
            "references A::B::C 7:4",
            "references Billing::ChargeCard 3:4",
            "references Config::DEFAULTS 8:4",
            "references Foo::Bar 9:4",
            "references Users::Create 6:4",
        ]
    );
}

// kernel Ruby parity: calls on a constructed receiver
#[test]
fn ruby_calls_on_constructed_receiver() {
    let src = [
        "class Callers",
        "  def a(key, klass)",
        "    CacheWriter.new.write_cache(key)",
        "    CacheWriter.new(key, ttl: 5).flush!",
        "    Billing::Ledger.new { |l| l }.post",
        "    ::CacheWriter.new.ready?",
        "    CacheWriter.new&.write_cache(key)",
        "    klass.new.run",
        "    self.class.new(key).run",
        "    Widget.create(key).save",
        "    CacheWriter.new.first.second",
        "  end",
        "end",
        "",
    ]
    .join("\n");
    let x = ex("src/callers.rb", &src);
    let got: Vec<String> = refs(&x, "calls")
        .iter()
        .filter(|r| r.name.contains('.') || r.name.contains('('))
        .map(|r| format!("{} {}", r.name, r.line))
        .collect();
    assert_eq!(
        sorted(got),
        [
            "CacheWriter.new.first.second 11",
            "Widget.create 10",
            "Widget.create(key).save 10",
            "klass.new 8",
            "klass.new.run 8",
            "new ::CacheWriter().ready? 6",
            "new Billing::Ledger().post 5",
            "new CacheWriter().first 11",
            "new CacheWriter().flush! 4",
            "new CacheWriter().write_cache 3",
            "new CacheWriter().write_cache 7",
            "self.class.new 9",
            "self.class.new(key).run 9",
        ]
    );
}

// kernel Ruby parity: class methods are static
#[test]
fn ruby_class_methods_are_static() {
    let src = [
        "class Widget",
        "  def build = self",
        "  def self.build(opts) = new",
        "  def Widget.named = 1",
        "  class << self",
        "    def make = new",
        "    def other",
        "      def nested = 2",
        "    end",
        "  end",
        "  def run",
        "    def inner = 3",
        "  end",
        "end",
        "def self.top = 4",
        "def plain = 5",
        "",
    ]
    .join("\n");
    let x = ex("src/widget.rb", &src);
    let statics: Vec<String> = x
        .nodes
        .iter()
        .filter(|n| n.kind == "method" || n.kind == "function")
        .map(|n| {
            format!(
                "{} {}:{} {}",
                n.kind,
                n.name,
                n.start_line,
                if n.is_static { "static" } else { "instance" }
            )
        })
        .collect();
    assert_eq!(
        sorted(statics),
        [
            "function inner:12 instance",
            "function nested:8 instance",
            "function plain:16 instance",
            "function top:15 static",
            "method build:2 instance",
            "method build:3 static",
            "method make:6 static",
            "method named:4 static",
            "method other:7 static",
            "method run:11 instance",
        ]
    );
}

// kernel Ruby parity: mixin implements refs carry filePath through the v2
// ref-flag wire path (the ref itself; jouet has no per-ref file path)
#[test]
fn ruby_mixin_implements_ref() {
    let x = ex("src/widget.rb", "class Widget\n  include Comparable\nend\n");
    assert_eq!(
        ref_names(&x, "implements").first().map(String::as_str),
        Some("Comparable")
    );
}

// ---------------------------------------------------------------------------
// C / C++
// ---------------------------------------------------------------------------

// Import Extraction > C/C++ return type capture (#645): captures the
// normalized return type of a C++ method/function
#[test]
fn cpp_return_type_capture() {
    let x = ex(
        "f.cpp",
        r#"
struct Widget { void draw(); };
class Factory { public: static Widget create(); };
Widget Factory::create() { return Widget(); }
void doNothing() {}
"#,
    );
    let create = x
        .nodes
        .iter()
        .find(|n| n.name == "create" && (n.kind == "method" || n.kind == "function"))
        .expect("create");
    assert_eq!(create.return_type.as_deref(), Some("Widget"));
    assert_eq!(named(&x, "doNothing").expect("doNothing").return_type, None);
}

// Import Extraction > C/C++ return type capture (#645): unwraps a
// smart-pointer return type to its pointee
#[test]
fn cpp_smart_pointer_return_type() {
    let x = ex(
        "f.cpp",
        r#"
#include <memory>
struct Widget {};
std::unique_ptr<Widget> makeWidget() { return nullptr; }
"#,
    );
    assert_eq!(
        named(&x, "makeWidget")
            .expect("makeWidget")
            .return_type
            .as_deref(),
        Some("Widget")
    );
}

// Import Extraction > C/C++ single-argument function macros (#1373):
// recovers single-argument function macros in %s
#[test]
fn c_cpp_single_argument_function_macros() {
    let code = "#define NATIVE_FN(name) int name(void)\nNATIVE_FN(get_version) { return helper(); }\nint use_it(void) { return get_version(); }\n";
    for lang in ["c", "cpp"] {
        let x = ex(&format!("main.{lang}"), code);
        let fns: Vec<&Node> = of_kind(&x, "function").collect();
        assert_eq!(
            fns.iter().map(|n| n.name.as_str()).collect::<Vec<_>>(),
            ["get_version", "use_it"],
            "{lang}"
        );
        assert_eq!(fns[0].qualified_name, "get_version");
        assert_eq!(
            (fns[0].start_line, fns[0].end_line, fns[0].start_column),
            (2, 2, 0)
        );
        assert_eq!(refs_from(&x, fns[0], "calls"), ["helper"], "{lang}");
        assert_eq!(refs_from(&x, fns[1], "calls"), ["get_version"], "{lang}");
    }
}

// Import Extraction > C/C++ single-argument function macros (#1373): does
// not guess single-argument macro names in %s
#[test]
fn c_cpp_single_argument_macros_not_guessed() {
    for lang in ["c", "cpp"] {
        let file = format!("main.{lang}");
        for prefix in [
            "",
            "// #define NATIVE_FN(name) int name(void)\n",
            "#define NATIVE_FN(name) int fixed(name)\n",
            "#define NATIVE_FN(name) int test_ ## name(void)\n",
            "#define NATIVE_FN(name) register_test(name)\n",
            "#define NATIVE_FN(name) typedef int name(void)\n",
            "#define NATIVE_FN(name) int name(void)\n#define NATIVE_FN int\n",
            "#define NATIVE_FN(name) int name(void)\n#ifdef OTHER\n#undef NATIVE_FN\n#endif\n",
            "#define NATIVE_FN(name) int name(void)\n#undef NATIVE_FN\n",
            "#define NATIVE_FN(name) int name(void)\n#define NATIVE_FN(name) int fixed(name)\n",
        ] {
            let x = ex(
                &file,
                &format!("{prefix}NATIVE_FN(candidate) {{ return 1; }}\n"),
            );
            assert!(
                find(&x, "function", "candidate").is_none(),
                "{lang}: {prefix:?}"
            );
        }
        let alternate = [
            "#ifdef OTHER",
            "#define NATIVE_FN(name) int name(void)",
            "#else",
            "NATIVE_FN(candidate) { return 1; }",
            "#endif",
            "",
        ]
        .join("\n");
        assert!(find(&ex(&file, &alternate), "function", "candidate").is_none());
        let ordinary = ex(&file, "int (parenthesized)(void) { return 1; }\n");
        assert_eq!(
            of_kind(&ordinary, "function")
                .next()
                .map(|n| n.name.as_str()),
            Some("(parenthesized)")
        );
        if lang == "c" {
            let knr = ex("knr.c", "int old_style(arg) int arg; { return arg; }\n");
            assert_eq!(
                of_kind(&knr, "function").next().map(|n| n.name.as_str()),
                Some("old_style")
            );
        }
    }
}

// kernel C/C++ parity: single-argument function macros and negative
// controls: %s (#1373)
#[test]
fn c_cpp_single_argument_macro_fixture() {
    let source = [
        "#define NATIVE_FN(name) int name(void)",
        "NATIVE_FN(get_version) { return helper(); }",
        "int use_it(void) { return get_version(); }",
        "#define FN(name) int name(void)",
        "FN(short_macro) { return 0; }",
        "#define POINTER_FN(name) const char *name(void)",
        "POINTER_FN(get_text) { return 0; }",
        "#define TEST_CASE(name) int test_ ## name(void)",
        "TEST_CASE(candidate) { return 0; }",
        "#define REGISTER_FN(name) register_test(name)",
        "REGISTER_FN(registration) { return 0; }",
        "int (parenthesized)(void) { return 1; }",
        "",
    ]
    .join("\n");
    for lang in ["c", "cpp"] {
        for src in [source.clone(), crlf(&source)] {
            let fns = names(&ex(&format!("fixtures/macros.{lang}"), &src), "function");
            for name in ["get_version", "get_text", "short_macro"] {
                assert!(fns.iter().any(|f| f == name), "{lang}: {name} in {fns:?}");
            }
            for name in ["candidate", "registration"] {
                assert!(!fns.iter().any(|f| f == name), "{lang}: {name} in {fns:?}");
            }
        }
    }
}

// Import Extraction > C++ namespace qualifiedName prefixing: prefixes
// contained symbols and handles nesting; anonymous stays bare
#[test]
fn cpp_namespace_prefixes() {
    let x = ex(
        "dispatch.cpp",
        r#"namespace flash {
namespace detail {
void helper() {}
}
void compute_attn(int x) { detail::helper(); }
class Softmax {
public:
    void rescale() {}
};
}
namespace {
void file_local() {}
}
void global_fn() { flash::compute_attn(1); }
"#,
    );
    assert!(!x.has_errors);
    let qn = |name: &str| named(&x, name).map(|n| n.qualified_name.clone());
    assert_eq!(qn("compute_attn").as_deref(), Some("flash::compute_attn"));
    assert_eq!(qn("helper").as_deref(), Some("flash::detail::helper"));
    assert_eq!(qn("Softmax").as_deref(), Some("flash::Softmax"));
    assert_eq!(qn("rescale").as_deref(), Some("flash::Softmax::rescale"));
    assert_eq!(qn("file_local").as_deref(), Some("file_local"));
    assert_eq!(qn("global_fn").as_deref(), Some("global_fn"));
    let calls = ref_names(&x, "calls");
    assert!(calls.contains(&"flash::compute_attn".to_string()));
    assert!(calls.contains(&"detail::helper".to_string()));
}

// Import Extraction > C++ namespace qualifiedName prefixing: C++17 nested
// namespace form prefixes as written
#[test]
fn cpp_nested_namespace_form() {
    let x = ex("nested.cpp", "namespace a::b {\nint f() { return 1; }\n}\n");
    assert_eq!(named(&x, "f").expect("f").qualified_name, "a::b::f");
}

// Import Extraction > C++ namespace qualifiedName prefixing: out-of-line
// method definitions inside a namespace carry the namespace prefix
#[test]
fn cpp_out_of_line_method_in_namespace() {
    let x = ex(
        "manifest_startup.cpp",
        r#"namespace simulator {
class ManifestStartup {
public:
    struct Input { int x; };
    struct Output { int y; };
    static Output Apply(const Input& input);
};
ManifestStartup::Output ManifestStartup::Apply(const Input& input) { return {}; }
}
"#,
    );
    assert!(
        x.nodes
            .iter()
            .any(|n| n.name == "Apply" && n.qualified_name == "simulator::ManifestStartup::Apply")
    );
    assert_eq!(
        of_kind(&x, "class").next().expect("class").qualified_name,
        "simulator::ManifestStartup"
    );
}

// Import Extraction > C++ namespace qualifiedName prefixing: a receiver that
// re-spells the namespace path is not double-prefixed
#[test]
fn cpp_respelled_namespace_not_double_prefixed() {
    let x = ex(
        "m.cpp",
        r#"namespace sim {
class M { public: static void f(); static void g(); };
void sim::M::f() {}
void M::g() {}
}
void sim::M::f2() {}
"#,
    );
    let qns: Vec<&str> = of_kind(&x, "method")
        .map(|n| n.qualified_name.as_str())
        .collect();
    for qn in ["sim::M::f", "sim::M::g", "sim::M::f2"] {
        assert!(qns.contains(&qn), "{qn} in {qns:?}");
    }
    assert!(!qns.iter().any(|q| q.contains("sim::sim")));
}

// Import Extraction > C++ forward declarations do not mint phantom class
// nodes (#1093): keeps only the real definition, dropping repeated forward
// decls
#[test]
fn cpp_forward_declarations_dropped() {
    let x = ex(
        "character.cpp",
        r#"
class APXCharacter;   // forward decl (header 1)
class APXCharacter;   // forward decl (header 2)
friend class APXCharacter;   // elaborated / friend forward reference

class APXCharacter {  // the one real definition
  int hp;
  void takeDamage(int amount) { hp -= amount; }
};
"#,
    );
    let classes: Vec<&Node> = of_kind(&x, "class")
        .filter(|n| n.name == "APXCharacter")
        .collect();
    assert_eq!(classes.len(), 1);
    assert_eq!(classes[0].start_line, 6);
    node(&x, "method", "takeDamage");
}

// Import Extraction > C++ forward declarations do not mint phantom class
// nodes (#1093): elaborated type references in declarations create no
// phantom class
#[test]
fn cpp_elaborated_type_reference_no_class() {
    let x = ex(
        "use.cpp",
        "class Foo;\nvoid f() { class Foo *p = nullptr; (void)p; }\n",
    );
    assert!(find(&x, "class", "Foo").is_none());
}

/// Names of a C++ file's methods and functions.
fn cpp_fn_names(file: &str, code: &str) -> Vec<String> {
    ex(file, code)
        .nodes
        .into_iter()
        .filter(|n| n.kind == "method" || n.kind == "function")
        .map(|n| n.name)
        .collect()
}

// Import Extraction > C++ reference-return method/function names (#1093
// follow-up): names an inline reference-returning method by its identifier;
// handles rvalue-reference returns and reference-returning free functions;
// leaves pointer, value, and out-of-line reference returns unchanged
#[test]
fn cpp_reference_return_names() {
    let names = cpp_fn_names(
        "r.cpp",
        "class C {\npublic:\n  const int& getRef() const { return x; }\n  int& mutRef() { return x; }\n  int x;\n};",
    );
    assert!(names.contains(&"getRef".to_string()));
    assert!(names.contains(&"mutRef".to_string()));
    assert!(!names.iter().any(|n| n.contains(['&', '(', ')'])));

    let has = |code: &str, name: &str| cpp_fn_names("r.cpp", code).iter().any(|n| n == name);
    assert!(has("class C { int&& take() { return 1; } };", "take"));
    assert!(has(
        "const int& globalRef() { static int x; return x; }",
        "globalRef"
    ));
    assert!(has(
        "class C { int* getPtr() { return &x; } int x; };",
        "getPtr"
    ));
    assert!(has(
        "class C { int getVal() const { return x; } int x; };",
        "getVal"
    ));
    assert!(has("const int& C::getRef() const { return x; }", "getRef"));
}

// Import Extraction > C++ user-defined conversion operator names (#1093
// follow-up): names a conversion operator as "operator <type>"; handles a
// user-type conversion operator; leaves symbolic operator overloads
// unchanged
#[test]
fn cpp_conversion_operator_names() {
    let names = cpp_fn_names(
        "o.cpp",
        "struct S {\n  operator int() const { return 1; }\n  operator bool() { return true; }\n  int x;\n};",
    );
    assert!(names.contains(&"operator int".to_string()));
    assert!(names.contains(&"operator bool".to_string()));
    assert!(!names.iter().any(|n| n.contains('(') || n.contains("const")));

    assert!(cpp_fn_names(
        "o.cpp",
        "struct FALSMovementState {\n  operator EALSMovementState() const { return State; }\n  EALSMovementState State;\n};"
    )
    .contains(&"operator EALSMovementState".to_string()));

    let names = cpp_fn_names(
        "o.cpp",
        "struct S {\n  S operator+(const S& o) const { return o; }\n  int& operator[](int i) { return x; }\n  int x;\n};",
    );
    assert!(names.contains(&"operator+".to_string()));
    assert!(names.contains(&"operator[]".to_string()));
}

/// The call refs of a C++ body after the explicit-operator test header.
fn operator_call_refs(body: &str) -> Vec<String> {
    let header = "struct V {\n  V operator+(const V& o) const;\n  V operator[](int i) const;\n  V operator()(int i) const;\n  bool operator==(const V& o) const;\n  int get() const;\n};\n";
    ref_names(&ex("op.cpp", &format!("{header}{body}")), "calls")
}

// Import Extraction > C++ explicit operator-call refs (#1247): recovers
// receiver.operator+, pointer receivers, subscript/call/comparison forms;
// leaves plain member calls unchanged
#[test]
fn cpp_explicit_operator_call_refs() {
    let has = |refs: &[String], name: &str| refs.iter().any(|r| r == name);
    assert!(has(
        &operator_call_refs("V f(const V& a, const V& b) { return a.operator+(b); }\n"),
        "a.operator+"
    ));
    assert!(has(
        &operator_call_refs("V f(const V* p, const V& b) { return p->operator+(b); }\n"),
        "p.operator+"
    ));
    let refs = operator_call_refs(
        "V f1(const V& a) { return a.operator[](3); }\nV f2(V& a) { return a.operator()(1); }\nbool f3(const V& a, const V& b) { return a.operator==(b); }\n",
    );
    for name in ["a.operator[]", "a.operator()", "a.operator=="] {
        assert!(has(&refs, name), "{name} in {refs:?}");
    }
    assert!(has(
        &operator_call_refs("int f(const V& a) { return a.get(); }\n"),
        "a.get"
    ));
}

// Import Extraction > C++ explicit operator-call refs (#1247): normalizes
// spaced call-site operator names to the compact definition form
#[test]
fn cpp_spaced_operator_call_refs() {
    let refs = operator_call_refs(
        "bool f(const V& a, const V& b) { return a.operator == (b); }\nV g(const V& a) { return a.operator [] (3); }\n",
    );
    assert!(refs.contains(&"a.operator==".to_string()), "{refs:?}");
    assert!(refs.contains(&"a.operator[]".to_string()), "{refs:?}");
}

// Import Extraction > C++ explicit operator-call refs (#1247): drops the ref
// for a complex receiver instead of guessing (no wrong edge)
#[test]
fn cpp_complex_operator_receiver_dropped() {
    let refs = operator_call_refs(
        "struct W { V* obj(); };\nV f(W& w, const V& b) { return w.obj()->operator+(b); }\n",
    );
    assert!(!refs.iter().any(|r| r.contains("operator+")), "{refs:?}");
    assert!(refs.contains(&"w.obj".to_string()), "{refs:?}");
}

// Import Extraction > C++ explicit operator-call refs (#1247): emits the
// bare operator name for a this-> receiver
#[test]
fn cpp_this_operator_call_ref() {
    let x = ex(
        "op.cpp",
        "struct V {\n  V operator+(const V& o) const;\n  V twice() const { return this->operator+(*this); }\n};\n",
    );
    let refs = ref_names(&x, "calls");
    assert!(refs.contains(&"operator+".to_string()), "{refs:?}");
    assert!(!refs.iter().any(|r| r.contains("this")));
}

// Import Extraction > C++ templated base-class inheritance (#1043): strips
// template args from a templated base so the extends ref is the bare name
#[test]
fn cpp_templated_base_extends_bare_name() {
    let x = ex(
        "f.cpp",
        r#"
template<typename T> class Base {};
template<typename D> class CRTPBase {};
namespace ns { template<typename T> class Tpl {}; }
class Plain {};

class Widget : public Base<int> {};
class App : public CRTPBase<App> {};
class Q : public ns::Tpl<int> {};
class Both : public Base<char>, public Plain {};
"#,
    );
    let names = ref_names(&x, "extends");
    for name in ["Base", "CRTPBase", "ns::Tpl", "Plain"] {
        assert!(names.iter().any(|n| n == name), "{name} in {names:?}");
    }
    assert!(!names.iter().any(|n| n.contains('<')));
}

// Import Extraction > C++ out-of-line template method receivers (#1286):
// strips the template parameter list from the receiver qualifier
#[test]
fn cpp_out_of_line_template_receiver() {
    let x = ex(
        "box.cpp",
        r#"template <typename T>
class Box {
public:
    T get() const;
    void set(T v);
private:
    T value;
};

template <typename T> T Box<T>::get() const { return value; }
template <typename T> void Box<T>::set(T v) { value = v; }
"#,
    );
    assert!(!x.has_errors);
    let qns: Vec<&str> = of_kind(&x, "method")
        .map(|n| n.qualified_name.as_str())
        .collect();
    assert!(
        qns.contains(&"Box::get") && qns.contains(&"Box::set"),
        "{qns:?}"
    );
    assert!(!qns.iter().any(|q| q.contains('<')));
    let method_names = names(&x, "method");
    assert!(method_names.contains(&"get".to_string()) && method_names.contains(&"set".to_string()));
}

// Import Extraction > C++ out-of-line template method receivers (#1286):
// multi-line template parameter lists cannot leak into qualified_name
#[test]
fn cpp_multiline_template_receiver() {
    let x = ex(
        "capi_helper.h",
        r#"template <typename CType,
          typename CPPType,
          int32_t kMagicValidationSentinelConstantForTheHelperTemplateClassInstanceGuardLong>
class ApiHelper {
public:
    CPPType* validate();
};

template <typename CType,
          typename CPPType,
          int32_t kMagicValidationSentinelConstantForTheHelperTemplateClassInstanceGuardLong>
CPPType* ApiHelper<CType,
                   CPPType,
                   kMagicValidationSentinelConstantForTheHelperTemplateClassInstanceGuardLong>::validate() {
    return nullptr;
}
"#,
    );
    let validate = of_kind(&x, "method")
        .find(|n| n.name == "validate" && n.qualified_name.contains("::"))
        .expect("validate");
    assert_eq!(validate.qualified_name, "ApiHelper::validate");
    assert!(validate.qualified_name.len() < 255);
    assert!(!validate.qualified_name.contains(['<', '>', '\n']));
}

/// The instantiates refs of `code` wrapped in a C++ function body.
fn cpp_inst_names(code: &str) -> Vec<String> {
    ref_names(
        &ex("f.cpp", &format!("void run() {{\n{code}\n}}")),
        "instantiates",
    )
}

// Import Extraction > C++ stack-allocation construction (#1035): emits an
// instantiates ref for direct-init and brace-init; strips template args and
// namespace; does not emit for primitives, default construction, or the
// most-vexing parse; emits a single ref for a multi-declarator statement
#[test]
fn cpp_stack_allocation_instantiates() {
    assert_eq!(cpp_inst_names("Calculator calc(0);"), ["Calculator"]);
    assert_eq!(cpp_inst_names("Widget w{1, 2};"), ["Widget"]);
    assert_eq!(cpp_inst_names("std::vector<int> v(10);"), ["vector"]);
    assert_eq!(cpp_inst_names("ns::Widget w(0);"), ["Widget"]);
    for code in [
        "int x(5);",
        "int y{6};",
        "auto z = make();",
        "Calculator deferred;",
        "Calculator calc();",
    ] {
        assert!(cpp_inst_names(code).is_empty(), "{code}");
    }
    assert_eq!(cpp_inst_names("Calculator a(1), b(2);"), ["Calculator"]);
}

// Import Extraction > C++ stack-allocation construction (#1035): records
// constructor defaults and array element arities (#1839)
#[test]
fn cpp_constructor_defaults_and_array_arities() {
    let x = ex(
        "f.cpp",
        "struct Widget { Widget(int x = 1); };\nvoid run() { Widget a[2]; Widget b[3]{{2}, {3}}; }",
    );
    assert_eq!(
        of_kind(&x, "method")
            .next()
            .and_then(|n| n.signature.as_deref()),
        Some("(int x = 1);")
    );
    assert_eq!(
        ref_names(&x, "calls"),
        [
            "Widget::Widget/0",
            "Widget::Widget/1",
            "Widget::Widget/1",
            "Widget::Widget/0",
        ]
    );
}

// Import Extraction > C/C++ imports: system include, system include with
// path, local include, C header
#[test]
fn c_cpp_include_forms() {
    let i = first_import("main.cpp", "#include <iostream>");
    assert_eq!(i.name, "iostream");
    assert_eq!(sig(&i), "#include <iostream>");
    assert_eq!(
        first_import("app.cpp", "#include <nlohmann/json.hpp>").name,
        "nlohmann/json.hpp"
    );
    assert_eq!(
        first_import("main.cpp", "#include \"myheader.h\"").name,
        "myheader.h"
    );
    assert_eq!(first_import("main.c", "#include <stdio.h>").name, "stdio.h");
}

// Import Extraction > C/C++ imports: should extract multiple includes
#[test]
fn cpp_multiple_includes() {
    let x = ex(
        "app.cpp",
        "\n#include <iostream>\n#include <vector>\n#include \"config.h\"\n",
    );
    assert_eq!(
        sorted(names(&x, "import")),
        ["config.h", "iostream", "vector"]
    );
}

// Import Extraction > C/C++ imports: should create unresolved references for
// local includes / system includes
#[test]
fn cpp_include_import_refs() {
    let x = ex("main.cpp", "#include \"myheader.h\"");
    let r = refs(&x, "imports")
        .into_iter()
        .find(|r| r.name == "myheader.h")
        .expect("imports myheader.h");
    assert_eq!(r.line, 1);
    assert!(has_ref(
        &ex("main.cpp", "#include <iostream>"),
        "imports",
        "iostream"
    ));
}

// C++ pure-virtual method nodes (#1727): indexes Store::read from the issue
// fixture and records the call
#[test]
fn cpp_pure_virtual_methods() {
    let x = ex(
        "store.cc",
        r#"
class Store {
public:
    virtual ~Store() {}
    virtual int read(int key) = 0;
};

class DiskStore : public Store {
public:
    int read(int key) override { return key + 1; }
};

class MemStore : public Store {
public:
    int read(int key) override { return key + 2; }
};

int fetch(Store* s, int k) {
    return s->read(k);
}
"#,
    );
    let qns: Vec<&str> = of_kind(&x, "method")
        .map(|n| n.qualified_name.as_str())
        .collect();
    for qn in ["Store::read", "DiskStore::read", "MemStore::read"] {
        assert!(qns.contains(&qn), "{qn} in {qns:?}");
    }
    let base = x
        .nodes
        .iter()
        .find(|n| n.qualified_name == "Store::read")
        .unwrap();
    assert!(base.is_abstract);
    assert!(
        ref_names(&x, "calls")
            .iter()
            .any(|r| r == "read" || r.ends_with(".read") || r.ends_with("->read") || r == "s.read")
    );
}

// C++ pure-virtual method nodes (#1727): indexes pure virtuals with
// pointer/reference return types and operators
#[test]
fn cpp_pure_virtual_pointer_reference_operators() {
    let x = ex(
        "clone.hpp",
        r#"
class Cloneable {
public:
    virtual Cloneable* clone() = 0;
    virtual const Foo& get() = 0;
    virtual Cloneable& operator=(const Cloneable&) = 0;
    int notPure(int x);
    int data = 0;
};
"#,
    );
    let methods = names(&x, "method");
    for name in ["clone", "get", "operator="] {
        assert!(methods.iter().any(|m| m == name), "{name} in {methods:?}");
    }
    for name in ["notPure", "data"] {
        assert!(!methods.iter().any(|m| m == name), "{name} in {methods:?}");
    }
    assert!(named(&x, "clone").unwrap().is_abstract);
}

// C/C++ union declarations: extracts a named union as a type node, but not a
// forward declaration
#[test]
fn c_named_union_not_forward_declaration() {
    let x = ex(
        "packet.c",
        r#"
union packet_hdr {
  unsigned int raw;
  unsigned short port;
};

/* forward declaration — not a definition */
union opaque_hdr;

static unsigned int hdr_raw(union packet_hdr *h) { return h->raw; }
"#,
    );
    assert_eq!(named(&x, "packet_hdr").expect("packet_hdr").kind, "union");
    assert!(named(&x, "opaque_hdr").is_none());
    assert_eq!(x.nodes.iter().filter(|n| n.name == "packet_hdr").count(), 1);
}

// C/C++ union declarations: gives a typedef union the typedef name, not a
// second <anonymous> node
#[test]
fn c_typedef_union_named_by_typedef() {
    let x = ex(
        "word.c",
        "\ntypedef union {\n  unsigned int u;\n  float f;\n} word_t;\n",
    );
    assert_eq!(named(&x, "word_t").map(|n| n.kind), Some("union"));
    assert!(named(&x, "<anonymous>").is_none());
}

// C/C++ union declarations: extracts a C++ union with member functions
#[test]
fn cpp_union_with_member_functions() {
    let x = ex(
        "value.cpp",
        "\nunion Value {\n  int i;\n  double d;\n  int as_int() const { return i; }\n};\n",
    );
    let value = named(&x, "Value").expect("Value");
    assert_eq!(value.kind, "union");
    let as_int = named(&x, "as_int").expect("as_int");
    assert!(contains(&x, value, as_int));
}
