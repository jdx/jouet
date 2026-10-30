"""A small GraphQL executor over plain dicts, for the fake GitHub.

It is not a GraphQL server. It parses real queries (operations, variables,
aliases, arguments, fragments, inline fragments, @include and @skip) and
resolves each field by name against a tree of dicts, which is all a fixture
needs to answer what `gh` and `jouet` ask:

- a field is the dict key of the same name;
- a list asked for with `nodes`, `edges`, `totalCount` or `pageInfo` becomes a
  connection, with every item on one page;
- arguments other than paging filter a list by the item's fields (`states`
  matches `state`), and a singular field with arguments such as
  `pullRequest(number: 7)` picks the first match from its plural
  (`pullRequests`);
- `... on Type` matches an item's `__typename`;
- `__type(name:)` lists the fields the fixture's objects of that type have,
  which is all `gh` asks of introspection.

A field a fixture does not have resolves to null and is reported through
`missing`, so a report can show where the fake fell short instead of the
agent silently seeing less than GitHub would show.
"""

from __future__ import annotations

import re
from collections.abc import Callable
from dataclasses import dataclass, field
from typing import Any

# Arguments that page or order a connection rather than filter it.
PAGING = {"first", "last", "after", "before", "orderBy"}
# Interfaces every object is treated as implementing.
INTERFACES = {"Node", "Actor", "Comment", "UniformResourceLocatable", "RepositoryNode"}


class GraphQLError(Exception):
    pass


_TOKEN = re.compile(
    r"""
    (?P<skip>[\s,]+|\#[^\n]*)
  | (?P<spread>\.\.\.)
  | (?P<string>"(?:\\.|[^"\\])*")
  | (?P<number>-?\d+(?:\.\d+)?(?:[eE][+-]?\d+)?)
  | (?P<name>[_A-Za-z][_0-9A-Za-z]*)
  | (?P<punct>[!$():=@\[\]{}|&])
    """,
    re.VERBOSE,
)


def tokenize(text: str) -> list[tuple[str, str]]:
    tokens, pos = [], 0
    while pos < len(text):
        m = _TOKEN.match(text, pos)
        if not m:
            raise GraphQLError(f"unexpected character {text[pos]!r} at {pos}")
        pos = m.end()
        kind = m.lastgroup
        if kind != "skip":
            tokens.append((kind, m.group(kind)))
    return tokens


@dataclass
class Field:
    name: str
    alias: str
    args: dict[str, Any]
    directives: list[tuple[str, dict[str, Any]]]
    selections: list[Any] | None


@dataclass
class Spread:
    name: str
    directives: list[tuple[str, dict[str, Any]]]


@dataclass
class Inline:
    on: str | None
    directives: list[tuple[str, dict[str, Any]]]
    selections: list[Any]


@dataclass
class Variable:
    name: str


@dataclass
class Document:
    kind: str = "query"
    defaults: dict[str, Any] = field(default_factory=dict)
    selections: list[Any] = field(default_factory=list)
    fragments: dict[str, tuple[str, list[Any]]] = field(default_factory=dict)


class Parser:
    def __init__(self, text: str):
        self.tokens = tokenize(text)
        self.pos = 0

    def peek(self, value: str | None = None) -> bool:
        if self.pos >= len(self.tokens):
            return False
        return value is None or self.tokens[self.pos][1] == value

    def take(self, value: str | None = None) -> str:
        if self.pos >= len(self.tokens):
            raise GraphQLError("unexpected end of query")
        kind, text = self.tokens[self.pos]
        if value is not None and text != value:
            raise GraphQLError(f"expected {value!r}, found {text!r}")
        self.pos += 1
        return text

    def name(self) -> str:
        kind, text = self.tokens[self.pos] if self.pos < len(self.tokens) else ("", "")
        if kind != "name":
            raise GraphQLError(f"expected a name, found {text!r}")
        self.pos += 1
        return text

    def document(self) -> Document:
        doc = Document()
        operations = 0
        while self.peek():
            if self.peek("{"):
                doc.selections = self.selection_set()
                operations += 1
            elif self.peek("query") or self.peek("mutation") or self.peek("subscription"):
                doc.kind = self.take()
                if not self.peek("(") and not self.peek("{") and not self.peek("@"):
                    self.name()
                if self.peek("("):
                    doc.defaults = self.variable_definitions()
                self.directives()
                doc.selections = self.selection_set()
                operations += 1
            elif self.peek("fragment"):
                self.take()
                name = self.name()
                self.take("on")
                on = self.name()
                self.directives()
                doc.fragments[name] = (on, self.selection_set())
            else:
                raise GraphQLError(f"unexpected {self.tokens[self.pos][1]!r}")
        if operations != 1:
            raise GraphQLError("expected exactly one operation")
        return doc

    def variable_definitions(self) -> dict[str, Any]:
        defaults = {}
        self.take("(")
        while not self.peek(")"):
            self.take("$")
            name = self.name()
            self.take(":")
            self.type_ref()
            if self.peek("="):
                self.take()
                defaults[name] = self.value()
            self.directives()
        self.take(")")
        return defaults

    def type_ref(self) -> None:
        if self.peek("["):
            self.take()
            self.type_ref()
            self.take("]")
        else:
            self.name()
        if self.peek("!"):
            self.take()

    def directives(self) -> list[tuple[str, dict[str, Any]]]:
        out = []
        while self.peek("@"):
            self.take()
            name = self.name()
            out.append((name, self.arguments() if self.peek("(") else {}))
        return out

    def arguments(self) -> dict[str, Any]:
        args = {}
        self.take("(")
        while not self.peek(")"):
            name = self.name()
            self.take(":")
            args[name] = self.value()
        self.take(")")
        return args

    def value(self) -> Any:
        kind, text = self.tokens[self.pos]
        if text == "$":
            self.take()
            return Variable(self.name())
        if text == "[":
            self.take()
            items = []
            while not self.peek("]"):
                items.append(self.value())
            self.take("]")
            return items
        if text == "{":
            self.take()
            obj = {}
            while not self.peek("}"):
                key = self.name()
                self.take(":")
                obj[key] = self.value()
            self.take("}")
            return obj
        self.pos += 1
        if kind == "string":
            return _unquote(text)
        if kind == "number":
            return float(text) if any(c in text for c in ".eE") else int(text)
        if text in ("true", "false"):
            return text == "true"
        if text == "null":
            return None
        if kind == "name":
            return text  # an enum value
        raise GraphQLError(f"unexpected {text!r} in a value")

    def selection_set(self) -> list[Any]:
        self.take("{")
        out = []
        while not self.peek("}"):
            if self.peek("..."):
                self.take()
                if self.peek("on"):
                    self.take()
                    on = self.name()
                    out.append(Inline(on, self.directives(), self.selection_set()))
                elif self.peek("{") or self.peek("@"):
                    out.append(Inline(None, self.directives(), self.selection_set()))
                else:
                    out.append(Spread(self.name(), self.directives()))
                continue
            alias = name = self.name()
            if self.peek(":"):
                self.take()
                name = self.name()
            args = self.arguments() if self.peek("(") else {}
            directives = self.directives()
            selections = self.selection_set() if self.peek("{") else None
            out.append(Field(name, alias, args, directives, selections))
        self.take("}")
        return out


def _unquote(text: str) -> str:
    import json

    return json.loads(text)


def parse(text: str) -> Document:
    return Parser(text).document()


# A mutation resolver: (arguments, executor) -> the payload object.
Mutation = Callable[[dict[str, Any], "Executor"], Any]


class Executor:
    def __init__(
        self,
        root: dict[str, Any],
        mutations: dict[str, Mutation] | None = None,
    ):
        self.root = root
        self.mutations = mutations or {}
        self.missing: list[str] = []
        self.doc = Document()
        self.variables: dict[str, Any] = {}

    def execute(self, query: str, variables: dict[str, Any] | None = None) -> dict[str, Any]:
        """Returns a GraphQL response: `{"data": ...}` or `{"errors": [...]}`."""
        try:
            self.doc = parse(query)
            self.variables = {**self.doc.defaults, **(variables or {})}
            if self.doc.kind == "mutation":
                data = {}
                for f in self.collect(self.doc.selections, "Mutation"):
                    resolver = self.mutations.get(f.name)
                    if resolver is None:
                        raise GraphQLError(f"the fake GitHub has no mutation {f.name}")
                    payload = resolver(self.args(f.args), self)
                    data[f.alias] = self.select(payload, f.selections, f.name)
                return {"data": data}
            return {"data": self.select(self.root, self.doc.selections, "Query")}
        except GraphQLError as e:
            return {"errors": [{"message": str(e)}]}

    def args(self, raw: Any) -> Any:
        if isinstance(raw, Variable):
            return self.variables.get(raw.name)
        if isinstance(raw, list):
            return [self.args(v) for v in raw]
        if isinstance(raw, dict):
            return {k: self.args(v) for k, v in raw.items()}
        return raw

    def included(self, directives: list[tuple[str, dict[str, Any]]]) -> bool:
        for name, args in directives:
            cond = bool(self.args(args.get("if")))
            if (name == "include" and not cond) or (name == "skip" and cond):
                return False
        return True

    def collect(self, selections: list[Any], typename: str | None) -> list[Field]:
        out = []
        for s in selections:
            if not self.included(s.directives):
                continue
            if isinstance(s, Field):
                out.append(s)
            elif isinstance(s, Inline):
                if s.on is None or matches(s.on, typename):
                    out.extend(self.collect(s.selections, typename))
            else:
                on, body = self.doc.fragments.get(s.name, (None, []))
                if on is None:
                    raise GraphQLError(f"unknown fragment {s.name}")
                if matches(on, typename):
                    out.extend(self.collect(body, typename))
        return out

    def select(self, obj: Any, selections: list[Any] | None, where: str) -> Any:
        if obj is None or selections is None:
            return strip(obj)
        if isinstance(obj, list):
            return [self.select(item, selections, where) for item in obj]
        if not isinstance(obj, dict):
            return obj
        typename = obj.get("__typename")
        out: dict[str, Any] = {}
        for f in self.collect(selections, typename):
            value = self.resolve(obj, f, typename or where)
            out[f.alias] = self.select(value, f.selections, f"{typename or where}.{f.name}")
        return out

    def resolve(self, obj: dict[str, Any], f: Field, where: str) -> Any:
        if f.name == "__typename":
            return obj.get("__typename", where.rsplit(".", 1)[-1])
        args = self.args(f.args)
        if f.name == "__type":
            return self.introspect(args.get("name", ""))
        filters = {k: v for k, v in args.items() if k not in PAGING}
        if f.name in obj:
            value = obj[f.name]
        elif filters and f"{f.name}s" in obj:
            items = items_of(obj[f"{f.name}s"])
            found = [i for i in items if matches_filters(i, filters)]
            return found[0] if found else None
        elif f.name == "node" and "id" in args:
            return find_node(self.root, args["id"])
        elif f.name == "nodes" and "ids" in args and obj is self.root:
            return [find_node(self.root, i) for i in args["ids"]]
        else:
            self.missing.append(f"{where}.{f.name}")
            return None
        if f.name not in obj or not (filters or wants_connection(f)):
            return value
        items = items_of(value)
        if items is None:
            return value
        if filters and isinstance(items, list):
            items = [i for i in items if matches_filters(i, filters)]
        if wants_connection(f):
            return connection(items)
        return items

    def introspect(self, name: str) -> dict[str, Any]:
        """`__type(name:)`, which `gh` asks to detect features: the fields
        the fixture's objects of that type have, so `gh` takes the paths the
        fixture can answer."""
        fields: dict[str, None] = {}

        def walk(tree: Any) -> None:
            if isinstance(tree, dict):
                if tree.get("__typename") == name:
                    fields.update(dict.fromkeys(k for k in tree if not k.startswith(("$", "__"))))
                for v in tree.values():
                    walk(v)
            elif isinstance(tree, list):
                for v in tree:
                    walk(v)

        walk(self.root)
        if name.endswith("Connection"):
            fields.update(dict.fromkeys(["nodes", "edges", "totalCount", "pageInfo"]))
        return {
            "__typename": "__Type",
            "name": name,
            "kind": "OBJECT",
            "fields": [{"__typename": "__Field", "name": n} for n in fields],
            "inputFields": [],
            "enumValues": [],
        }


def matches(on: str, typename: str | None) -> bool:
    return on in INTERFACES or typename is None or on == typename


def wants_connection(f: Field) -> bool:
    return f.selections is not None and any(
        isinstance(s, Field) and s.name in ("nodes", "edges", "totalCount", "pageInfo") for s in f.selections
    )


def items_of(value: Any) -> list[Any] | None:
    if isinstance(value, list):
        return value
    if isinstance(value, dict) and isinstance(value.get("nodes"), list):
        return value["nodes"]
    return None


def connection(items: list[Any]) -> dict[str, Any]:
    return {
        "nodes": items,
        "edges": [{"node": i, "cursor": f"cursor{n}"} for n, i in enumerate(items)],
        "totalCount": len(items),
        "pageInfo": {
            "hasNextPage": False,
            "hasPreviousPage": False,
            "endCursor": f"cursor{len(items) - 1}" if items else None,
            "startCursor": "cursor0" if items else None,
        },
    }


def matches_filters(item: Any, filters: dict[str, Any]) -> bool:
    if not isinstance(item, dict):
        return False
    for key, want in filters.items():
        if key == "states" and isinstance(want, list):
            if item.get("state") not in want:
                return False
        elif key in item and item[key] != want:
            return False
    return True


def find_node(tree: Any, node_id: str) -> Any:
    if isinstance(tree, dict):
        if tree.get("id") == node_id and "__typename" in tree:
            return tree
        values = tree.values()
    elif isinstance(tree, list):
        values = tree
    else:
        return None
    for v in values:
        found = find_node(v, node_id)
        if found is not None:
            return found
    return None


def strip(value: Any) -> Any:
    """A value with the fixture's private `$` keys removed."""
    if isinstance(value, dict):
        return {k: strip(v) for k, v in value.items() if not k.startswith("$")}
    if isinstance(value, list):
        return [strip(v) for v in value]
    return value
