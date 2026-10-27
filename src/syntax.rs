//! What a module's text alone says, without type checking it: its
//! declarations (types, traits, `impl` blocks, methods, fields,
//! constructors and constants), with their kinds, names, ranges, signatures
//! as the source writes them and whether they're public, and its imports.
//! Document symbols, workspace symbols, hover's declarations and
//! completion's imports come from here (docs/lsp.md).
//!
//! It reads the upstream parser's syntax tree. Text that doesn't parse is
//! read from a copy with the lines in the way blank (`outline`): the line
//! of a bracket nothing closes, then the lines the parser stops at, until
//! a copy parses. Blank lines keep every other line where it is, so the
//! ranges of what's left are the source's own.
use crate::position::Lines;
use location::Location;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::ops::Range;
use std::path::PathBuf;

/// What a declaration declares.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Kind {
    /// A type that isn't an enum: `type`, `type async`, `type copy`,
    /// `type extern` and so on.
    Type,
    /// `type enum`.
    Enum,
    Trait,
    /// An `impl` block, of a trait for a type (`impl ToString for Counter`)
    /// or reopening a type (`impl Counter`). It declares nothing itself.
    Implementation,
    /// A method of a type, trait or `impl` block, other than a static one.
    Method,
    /// `fn static` in a type or `impl` block.
    StaticMethod,
    /// A method defined at the top of a module, or an external function
    /// (`fn extern`).
    ModuleMethod,
    Field,
    /// An enum's constructor (`case Some(T)`).
    Constructor,
    Constant,
}

impl Kind {
    /// The kind's name, for messages and tests.
    pub fn name(self) -> &'static str {
        match self {
            Kind::Type => "type",
            Kind::Enum => "enum",
            Kind::Trait => "trait",
            Kind::Implementation => "impl",
            Kind::Method => "method",
            Kind::StaticMethod => "static method",
            Kind::ModuleMethod => "module method",
            Kind::Field => "field",
            Kind::Constructor => "constructor",
            Kind::Constant => "constant",
        }
    }

    /// Whether it can be imported from its module: `import m (Name)`.
    pub fn importable(self) -> bool {
        matches!(
            self,
            Kind::Type | Kind::Enum | Kind::Trait | Kind::ModuleMethod | Kind::Constant
        )
    }
}

/// A declaration, in bytes of the text it's in.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Declaration {
    pub kind: Kind,
    /// Its name as code uses it: `add`, `Counter`, `count` for the field
    /// `@count`, `+` for an operator. For an `impl` block, its header
    /// (`signature`) on one line.
    pub name: String,
    /// Where its name is, as the compiler locates it: the field's `@` is
    /// part of it. For an `impl` block, the type's name.
    pub name_range: Range<usize>,
    /// The whole declaration, as the compiler locates it: from its first
    /// keyword to its end, without its doc comment.
    pub range: Range<usize>,
    /// The declaration as the source writes it, up to its body, with the
    /// indentation of its first line taken off the others: `fn pub
    /// get(index: Int) -> Result[ref T, OutOfBounds]`, `type pub Array[T]`,
    /// `let pub @count: Int`, `case Some(T)`, `let pub MAX = 10`.
    pub signature: String,
    /// Whether it's `pub`. Constructors are as public as their type, and
    /// `impl` blocks count as public.
    pub public: bool,
    /// Its doc comment, as `inko doc` reads it (`Comments`): the text of
    /// the comments right above it, a line each, without their `#` and
    /// the space after it. `None` if it has none.
    pub documentation: Option<String>,
    /// Where it's written about: from the start of its doc comment's first
    /// line (or its own start) to the end of its signature. Hover reads
    /// this text.
    pub header: Range<usize>,
    /// What it contains: a type's fields, constructors and methods, a
    /// trait's or `impl` block's methods.
    pub children: Vec<Declaration>,
}

impl Declaration {
    /// Its name in the path of what's in it (`std.string.String.size`):
    /// an `impl` block's is its type's.
    pub fn owner(&self) -> &str {
        if self.kind != Kind::Implementation {
            return &self.name;
        }

        // `impl Trait[A] for Type[B] if ...`, or `impl Type if ...`.
        let rest = self.name.strip_prefix("impl ").unwrap_or(&self.name);
        let typ = rest.split_once(" for ").map_or(rest, |(_, t)| t);

        typ.split([' ', '[']).next().unwrap_or("")
    }

    /// The signature on one line: each line trimmed and joined with a
    /// space, except right inside brackets, and without a trailing comma
    /// before a closing bracket.
    pub fn detail(&self) -> String {
        one_line(&self.signature)
    }
}

/// A symbol in an import's list: its name, its alias, and its range (with
/// the alias).
pub type ImportSymbol = (String, Option<String>, Range<usize>);

/// An import, in bytes of the text it's in.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Import {
    /// The module's name, split: `["std", "stdio"]`.
    pub path: Vec<String>,
    /// The whole import.
    pub range: Range<usize>,
    /// The symbols in its list, if it has one (`(Stdout, STDIN as IN)`):
    /// each name, its alias, and its range (with the alias).
    pub symbols: Option<Vec<ImportSymbol>>,
    /// Where the list's brackets are, `(` to `)` included.
    pub list: Option<Range<usize>>,
    /// Whether it has build tags (`if linux`).
    pub tagged: bool,
}

impl Import {
    /// The names the import binds: the module's last name (unless a list
    /// leaves it out: `self` keeps it), or each symbol's alias or name.
    pub fn binds(&self) -> Vec<String> {
        match &self.symbols {
            None => self.path.last().cloned().into_iter().collect(),
            Some(symbols) => symbols
                .iter()
                .map(|(name, alias, _)| match (name.as_str(), alias) {
                    (_, Some(alias)) => alias.clone(),
                    ("self", None) => self.path.last().cloned().unwrap_or_default(),
                    (name, None) => name.to_string(),
                })
                .collect(),
        }
    }
}

/// A module's declarations and imports, read from its text.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Outline {
    /// In the order the text has them.
    pub declarations: Vec<Declaration>,
    pub imports: Vec<Import>,
    /// The first and last line (0-based) of each comment directly above a
    /// top-level item, and the line the item starts on, for placing new
    /// imports before a doc comment.
    pub items: Vec<(Option<usize>, usize)>,
    /// The lines (0-based) left blank because the text didn't parse
    /// (`outline`). Empty if it parsed as it is.
    pub blank: Vec<usize>,
    /// The module's doc comment, as `inko doc` reads it: the comments on
    /// the lines at the start of the file, one after another from the
    /// first (compiler/src/hir.rs:1320-1340).
    pub documentation: Option<String>,
}

impl Outline {
    /// Every declaration, depth first, with the declarations it's in
    /// (outermost first).
    pub fn all(&self) -> Vec<(&Declaration, Vec<&Declaration>)> {
        fn walk<'a>(
            list: &'a [Declaration],
            parents: &mut Vec<&'a Declaration>,
            out: &mut Vec<(&'a Declaration, Vec<&'a Declaration>)>,
        ) {
            for d in list {
                out.push((d, parents.clone()));
                parents.push(d);
                walk(&d.children, parents, out);
                parents.pop();
            }
        }

        let mut out = Vec::new();

        walk(&self.declarations, &mut Vec::new(), &mut out);
        out
    }

    /// The outline of `copy` (`text` with lines blank) as one of `text`:
    /// each byte in a line that isn't blank is at the same column of the
    /// same line, and one in a blank line at its start.
    fn moved(mut self, copy: &str, text: &str) -> Outline {
        let starts = |t: &str| -> Vec<usize> {
            std::iter::once(0)
                .chain(t.match_indices('\n').map(|(i, _)| i + 1))
                .collect()
        };
        let (from, to) = (starts(copy), starts(text));
        let map = |b: usize| -> usize {
            let line = from.partition_point(|s| *s <= b) - 1;
            let end = to.get(line + 1).map_or(text.len(), |n| n - 1);

            (to[line] + (b - from[line])).min(end.max(to[line]))
        };
        let range = |r: &mut Range<usize>| *r = map(r.start)..map(r.end);

        fn declarations(list: &mut [Declaration], range: &impl Fn(&mut Range<usize>)) {
            for d in list {
                range(&mut d.range);
                range(&mut d.name_range);
                range(&mut d.header);
                declarations(&mut d.children, range);
            }
        }

        declarations(&mut self.declarations, &range);

        for i in &mut self.imports {
            range(&mut i.range);

            if let Some(list) = &mut i.list {
                range(list);
            }

            for (_, _, r) in i.symbols.iter_mut().flatten() {
                range(r);
            }
        }

        self
    }

    /// The outline without the declarations whose signatures have a blank
    /// line in them (and what's in those): their signatures aren't the
    /// source's.
    fn without_blank_signatures(mut self, text: &str, blank: &BTreeSet<usize>) -> Outline {
        fn keep(list: &mut Vec<Declaration>, text: &str, blank: &BTreeSet<usize>) {
            list.retain(|d| {
                let first = text[..d.range.start].matches('\n').count();

                (first..first + d.signature.lines().count()).all(|l| !blank.contains(&l))
            });

            for d in list {
                keep(&mut d.children, text, blank);
            }
        }

        keep(&mut self.declarations, text, blank);
        self
    }

    /// The outline (of `copy`, moved to `text`) with only the top-level
    /// declarations nested in the text as in the copy: each of their lines
    /// has the same depth of braces (`depths`) in both, relative to their
    /// first, and the text isn't deeper where they start (inside a brace
    /// a blank line opened). A blank line that opened or closed a brace
    /// may have moved others in or out of another. Nor any after a blank
    /// line at the top of the copy that starts a type, trait or `impl`
    /// block in the text: its `{` may be missing, and what follows it in
    /// it.
    fn nested_as_in(mut self, text: &str, copy: &str, blank: &BTreeSet<usize>) -> Outline {
        let (ours, theirs) = (depths(text), depths(copy));
        let line_of = |byte: usize| text[..byte].matches('\n').count();
        let lines: Vec<&str> = text.split('\n').collect();
        let header = blank
            .iter()
            .filter(|b| {
                theirs.get(**b) == Some(&0)
                    && !self
                        .declarations
                        .iter()
                        .any(|d| (line_of(d.range.start)..=line_of(d.range.end)).contains(b))
                    && lines.get(**b).is_some_and(|l| {
                        let first = l.split_whitespace().next().unwrap_or("");

                        matches!(first, "type" | "trait" | "impl")
                    })
            })
            .min()
            .copied();

        self.declarations.retain(|d| {
            let (first, last) = (line_of(d.range.start), line_of(d.range.end));
            let (Some(o), Some(c)) = (ours.get(first), theirs.get(first)) else {
                return false;
            };

            header.is_none_or(|h| first < h)
                && o <= c
                && (first..=last).all(|l| match (ours.get(l), theirs.get(l)) {
                    (Some(a), Some(b)) => a - o == b - c,
                    _ => false,
                })
        });
        self
    }

    /// The innermost declaration whose name starts at `start`, and the
    /// declarations it's in.
    pub fn named_at(&self, start: usize) -> Option<(&Declaration, Vec<&Declaration>)> {
        self.all()
            .into_iter()
            .rfind(|(d, _)| d.name_range.start == start && d.kind != Kind::Implementation)
    }
}

/// What an `import` being typed needs next.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ImportContext {
    /// A module's name: the names before the last `.` (`["std"]` in
    /// `import std.fm`).
    Module(Vec<String>),
    /// A name in the symbol list of the module (`std.stdio`), and the
    /// names listed already.
    Symbol(String, Vec<String>),
}

/// What's being typed at the end of `before` (a text up to a position),
/// if it's an import: on the line of an `import` at the start of a line,
/// after its path's start, or in its symbol list, which may go over
/// lines. `None` for anything else, `import extern`, and an import's
/// build tags.
pub fn import_context(before: &str) -> Option<ImportContext> {
    let start = before
        .rfind("\nimport ")
        .map(|i| i + 1)
        .or_else(|| before.starts_with("import ").then_some(0))?;
    let rest = &before[start + "import ".len()..];
    let name = |s: &str| s.chars().all(|c| c.is_alphanumeric() || c == '_');

    match rest.split_once('(') {
        Some((path, list)) => {
            let path = path.trim();

            if list.contains(')') || path.is_empty() || !path.split('.').all(name) {
                return None;
            }

            let listed: Vec<String> = list
                .split(',')
                .map(|s| s.split_whitespace().next().unwrap_or("").to_string())
                .collect();

            if !listed.iter().all(|n| name(n)) {
                return None;
            }

            let done = listed.len() - 1;

            Some(ImportContext::Symbol(
                path.to_string(),
                listed.into_iter().take(done).collect(),
            ))
        }
        // A path is names and dots: an import of a path that's done, or
        // `import extern "..."`, has more.
        None => {
            let segments: Vec<&str> = rest.split('.').collect();

            if !segments.iter().all(|s| name(s)) {
                return None;
            }

            Some(ImportContext::Module(
                segments[..segments.len() - 1]
                    .iter()
                    .map(|s| s.to_string())
                    .collect(),
            ))
        }
    }
}

/// The edit that imports `symbol` from `module` (`std.stdio`) into `text`
/// (outlined by `outline`), as `inko fmt` orders imports
/// (compiler/src/format.rs:578-620): the import paths in string order, and
/// in a symbol list `self` first, then the names in string order. Into the
/// module's import's list, if it has one (`import std.stdio (Stdin)`
/// becomes `import std.stdio (Stdin, Stdout)`); an import of the module
/// without one gets `(self, Stdout)`; or else a new import, in the first
/// group of imports, or before the first declaration (and its doc
/// comment), followed by a blank line. An import with build tags isn't
/// added to. The bytes to replace, and their replacement.
pub fn import_edit(
    text: &str,
    outline: &Outline,
    module: &str,
    symbol: &str,
) -> (Range<usize>, String) {
    let path: Vec<&str> = module.split('.').collect();
    let existing = outline
        .imports
        .iter()
        .find(|i| !i.tagged && i.path.iter().map(String::as_str).eq(path.iter().copied()));

    if let Some(import) = existing {
        let Some(symbols) = &import.symbols else {
            let end = import.range.end;

            return (end..end, format!(" (self, {})", symbol));
        };

        // Before the first name that sorts after it (`self` first).
        let after = symbols
            .iter()
            .find(|(name, _, _)| name != "self" && name.as_str() > symbol);

        return match (after, symbols.last()) {
            (Some((_, _, r)), _) => (r.start..r.start, format!("{}, ", symbol)),
            (None, Some((_, _, r))) => (r.end..r.end, format!(", {}", symbol)),
            (None, None) => {
                let at = import
                    .list
                    .as_ref()
                    .map_or(import.range.end, |l| l.start + 1);

                (at..at, symbol.to_string())
            }
        };
    }

    let line = format!("import {} ({})\n", module, symbol);
    let line_start = |at: usize| text[..at].rfind('\n').map_or(0, |i| i + 1);
    let next_line = |at: usize| text[at..].find('\n').map_or(text.len(), |i| at + i + 1);
    // The first group: imports on consecutive lines.
    let mut group: Vec<&Import> = Vec::new();

    for import in &outline.imports {
        if let Some(last) = group.last()
            && text[last.range.end..import.range.start]
                .matches('\n')
                .count()
                > 1
        {
            break;
        }

        group.push(import);
    }

    if let Some(first) = group.first() {
        let at = match group.iter().find(|i| i.path.join(".").as_str() > module) {
            Some(i) => line_start(i.range.start),
            None => next_line(group.last().unwrap_or(first).range.end),
        };
        let line = if at == text.len() && !text.ends_with('\n') {
            format!("\n{}", line.trim_end())
        } else {
            line
        };

        return (at..at, line);
    }

    // Before the first declaration, and its doc comment.
    let first = outline
        .items
        .first()
        .map(|(doc, at)| doc.unwrap_or(*at))
        .and_then(|l| {
            text.split_inclusive('\n')
                .take(l)
                .map(str::len)
                .sum::<usize>()
                .into()
        });

    match first {
        Some(at) => (at..at, format!("{}\n", line)),
        None if text.is_empty() => (0..0, line),
        None if text.ends_with('\n') => (text.len()..text.len(), format!("\n{}", line)),
        None => (text.len()..text.len(), format!("\n\n{}", line.trim_end())),
    }
}

/// How many lines the parser may stop at, in turn, before `outline` gives
/// up on a text.
const ATTEMPTS: usize = 16;

/// The outline of `text`. If it doesn't parse, the outline of a copy with
/// the lines in the way blank: the line of a bracket nothing closes
/// (`unclosed`), then also, in turn, the line the parser stops at or the
/// line before it, whichever gets the parser further, until a copy
/// parses. `None` if none does within `ATTEMPTS`.
pub fn outline(text: &str) -> Option<Outline> {
    if let Some(outline) = parse(text) {
        return Some(outline);
    }

    let mut lines = BTreeSet::new();

    if let Some(line) = unclosed(text) {
        lines.insert(line);
    }

    let mut error = parse_error_line(&blank(text, &lines));

    for _ in 0..ATTEMPTS {
        let Some(line) = error else {
            let copy = blank(text, &lines);

            return parse(&copy).map(|o| Outline {
                blank: lines.iter().copied().collect(),
                ..o.moved(&copy, text)
                    .without_blank_signatures(text, &lines)
                    .nested_as_in(text, &copy, &lines)
            });
        };
        // The parser often stops at the token after what's unfinished, on
        // the next line: the line before it may be the one in the way.
        let before = text
            .lines()
            .enumerate()
            .take(line)
            .filter(|(i, l)| !l.trim().is_empty() && !lines.contains(i))
            .map(|(i, _)| i)
            .last();
        // Whichever lets the parser get furthest: all the way, or to a
        // later line. The line it stops at on a tie (the last of the
        // greatest), since what's further on is in the way less often.
        let Some((chosen, next)) = [before, Some(line)]
            .into_iter()
            .flatten()
            .filter(|l| !lines.contains(l))
            .map(|l| {
                let mut with = lines.clone();

                with.insert(l);
                (l, parse_error_line(&blank(text, &with)))
            })
            .max_by_key(|(_, next)| next.unwrap_or(usize::MAX))
        else {
            break;
        };

        lines.insert(chosen);
        error = next;
    }

    None
}

/// The outline of `text`, or `None` if it doesn't parse.
pub fn parse(text: &str) -> Option<Outline> {
    use ast::nodes::TopLevelExpression as T;

    let module =
        ast::parser::Parser::with_comments(text.as_bytes().to_vec(), PathBuf::from("outline"))
            .parse()
            .ok()?;
    let mut builder = Builder {
        text,
        lines: Lines::new(text),
        outline: Outline::default(),
    };
    let mut comment: Option<(usize, usize)> = None;
    let mut comments = Comments::default();
    let mut expressions = module.expressions.iter().peekable();
    let mut last = 0;
    let mut doc: Vec<&str> = Vec::new();

    // The module's own: comments one line after another from the first.
    while let Some(T::Comment(c)) = expressions.peek()
        && c.location.line_start.wrapping_sub(last) == 1
    {
        doc.push(&c.value);
        last = c.location.line_start;
        expressions.next();
    }

    builder.outline.documentation = (!doc.is_empty()).then(|| doc.join("\n"));

    for e in expressions {
        let line = |l: &Location| l.line_start.saturating_sub(1) as usize;
        let end = |l: &Location| l.line_end.saturating_sub(1) as usize;

        if let T::Comment(c) = e {
            comments.push(c);

            // A comment continues the one above it on the next line.
            comment = match comment {
                Some((first, last)) if line(&c.location) == last + 1 => {
                    Some((first, end(&c.location)))
                }
                _ => Some((line(&c.location), end(&c.location))),
            };
            continue;
        }

        let at = line(ast::nodes::Node::location(e));
        let doc = comment
            .take()
            .filter(|(_, last)| *last + 1 == at)
            .map(|(first, _)| first);

        builder.outline.items.push((doc, at));
        builder.top(e, &mut comments);
    }

    Some(builder.outline)
}

/// The comments read so far that may document what comes next, as
/// upstream collects them (`Comments`, compiler/src/hir.rs:39-90): a
/// comment more than a line below the one before starts them over, and a
/// declaration takes them if it starts on the line after the last one,
/// and otherwise drops them. Imports and `impl` blocks don't look at them.
#[derive(Default)]
struct Comments<'a> {
    /// Each comment's first and last line, and its text.
    nodes: Vec<(u32, u32, &'a str)>,
}

impl<'a> Comments<'a> {
    fn push(&mut self, comment: &'a ast::nodes::Comment) {
        let end = comment.location.line_end;
        let last = self.nodes.last().map_or(0, |(_, line, _)| *line);

        if end.wrapping_sub(last) > 1 && !self.nodes.is_empty() {
            self.nodes.clear();
        }

        self.nodes
            .push((comment.location.line_start, end, &comment.value));
    }

    /// The documentation, and the line (1-based) it starts on.
    fn documentation_for(&mut self, location: &Location) -> Option<(String, u32)> {
        let take = self
            .nodes
            .last()
            .is_some_and(|(_, end, _)| location.line_start.wrapping_sub(*end) == 1);
        let doc = take.then(|| {
            let text = self
                .nodes
                .iter()
                .map(|(_, _, text)| *text)
                .collect::<Vec<_>>()
                .join("\n");

            (text, self.nodes[0].0)
        });

        self.nodes.clear();
        doc
    }
}

struct Builder<'a> {
    text: &'a str,
    lines: Lines<'a>,
    outline: Outline,
}

impl Builder<'_> {
    fn bytes(&self, l: &Location) -> Range<usize> {
        let at = |line: u32, column: u32| self.lines.offset(line, column).unwrap_or(0);

        at(l.line_start, l.column_start)..at(l.line_end, l.column_end + 1)
    }

    /// The declaration's text up to `body` (its start), or all of it, and
    /// where that ends.
    fn signature(&self, range: &Range<usize>, body: Option<&Location>) -> (String, usize) {
        let end = body.map_or(range.end, |b| self.bytes(b).start.max(range.start));
        let text = self.text[range.start..end].trim_end();

        (
            dedent(self.text, range.start, text),
            range.start + text.len(),
        )
    }

    fn declaration(
        &self,
        kind: Kind,
        location: &Location,
        name: (&str, &Location),
        body: Option<&Location>,
        public: bool,
        comments: &mut Comments,
    ) -> Declaration {
        let documentation = comments.documentation_for(location);
        let from = documentation
            .as_ref()
            .and_then(|(_, line)| self.lines.offset(*line, 1));
        let name_range = self.bytes(name.1);
        // A constructor without members is located at its `case` alone.
        let range = {
            let r = self.bytes(location);

            r.start..r.end.max(name_range.end)
        };

        let (signature, end) = self.signature(&range, body);

        Declaration {
            kind,
            name: name.0.to_string(),
            name_range,
            signature,
            header: from.unwrap_or(range.start)..end,
            range,
            public,
            documentation: documentation.map(|(text, _)| text),
            children: Vec::new(),
        }
    }

    fn method(
        &self,
        m: &ast::nodes::DefineMethod,
        kind: Kind,
        comments: &mut Comments,
    ) -> Declaration {
        let kind = match m.kind {
            ast::nodes::MethodKind::Static => Kind::StaticMethod,
            _ => kind,
        };

        self.declaration(
            kind,
            &m.location,
            (&m.name.name, &m.name.location),
            m.body.as_ref().map(|b| &b.location),
            m.public,
            comments,
        )
    }

    fn top<'a>(&mut self, e: &'a ast::nodes::TopLevelExpression, comments: &mut Comments<'a>) {
        use ast::nodes::{
            ImplementationExpression as I, TopLevelExpression as T, TraitExpression as R,
            TypeExpression as Y,
        };

        let declaration = match e {
            T::DefineMethod(m) => self.method(m, Kind::ModuleMethod, comments),
            T::DefineType(t) => {
                let kind = match t.kind {
                    ast::nodes::TypeKind::Enum => Kind::Enum,
                    _ => Kind::Type,
                };
                let mut d = self.declaration(
                    kind,
                    &t.location,
                    (&t.name.name, &t.name.location),
                    Some(&t.body.location),
                    t.public,
                    comments,
                );
                let mut members = Comments::default();

                for e in &t.body.values {
                    match e {
                        Y::DefineMethod(m) => {
                            d.children.push(self.method(m, Kind::Method, &mut members))
                        }
                        Y::DefineField(f) => d.children.push(self.declaration(
                            Kind::Field,
                            &f.location,
                            (&f.name.name, &f.name.location),
                            None,
                            f.public,
                            &mut members,
                        )),
                        Y::DefineConstructor(c) => d.children.push(self.declaration(
                            Kind::Constructor,
                            &c.location,
                            (&c.name.name, &c.name.location),
                            None,
                            t.public,
                            &mut members,
                        )),
                        Y::Comment(c) => members.push(c),
                    }
                }

                d
            }
            T::DefineTrait(t) => {
                let mut d = self.declaration(
                    Kind::Trait,
                    &t.location,
                    (&t.name.name, &t.name.location),
                    Some(&t.body.location),
                    t.public,
                    comments,
                );
                let mut members = Comments::default();

                for e in &t.body.values {
                    match e {
                        R::DefineMethod(m) => {
                            d.children.push(self.method(m, Kind::Method, &mut members))
                        }
                        R::Comment(c) => members.push(c),
                    }
                }

                d
            }
            T::ImplementTrait(t) => {
                let mut d = self.implementation(&t.location, &t.type_name, &t.body.location);

                let mut members = Comments::default();

                for e in &t.body.values {
                    match e {
                        I::DefineMethod(m) => {
                            d.children.push(self.method(m, Kind::Method, &mut members))
                        }
                        I::Comment(c) => members.push(c),
                    }
                }

                d
            }
            T::ReopenType(t) => {
                let mut d = self.implementation(&t.location, &t.type_name, &t.body.location);

                let mut members = Comments::default();

                for e in &t.body.values {
                    match e {
                        I::DefineMethod(m) => {
                            d.children.push(self.method(m, Kind::Method, &mut members))
                        }
                        I::Comment(c) => members.push(c),
                    }
                }

                d
            }
            T::DefineConstant(c) => self.declaration(
                Kind::Constant,
                &c.location,
                (&c.name.name, &c.name.location),
                None,
                c.public,
                comments,
            ),
            T::Import(i) => {
                let symbols = i.symbols.as_ref().map(|s| {
                    s.values
                        .iter()
                        .map(|v| {
                            (
                                v.name.clone(),
                                v.alias.as_ref().map(|a| a.name.clone()),
                                self.bytes(&v.location),
                            )
                        })
                        .collect()
                });

                self.outline.imports.push(Import {
                    path: i.path.steps.iter().map(|s| s.name.clone()).collect(),
                    range: self.bytes(&i.location),
                    symbols,
                    list: i.symbols.as_ref().map(|s| self.bytes(&s.location)),
                    tagged: i.tags.is_some(),
                });
                return;
            }
            T::ExternImport(_) | T::Comment(_) => return,
        };

        self.outline.declarations.push(declaration);
    }

    fn implementation(
        &self,
        location: &Location,
        typ: &ast::nodes::Constant,
        body: &Location,
    ) -> Declaration {
        let range = self.bytes(location);
        let (signature, end) = self.signature(&range, Some(body));

        Declaration {
            kind: Kind::Implementation,
            name: one_line(&signature),
            name_range: self.bytes(&typ.location),
            signature,
            header: range.start..end,
            range,
            public: true,
            documentation: None,
            children: Vec::new(),
        }
    }
}

/// `text` (which starts at byte `start` of `source`) with the indentation
/// of the line it starts on taken off its other lines.
fn dedent(source: &str, start: usize, text: &str) -> String {
    let line_start = source[..start].rfind('\n').map_or(0, |i| i + 1);
    let indent = &source[line_start..start];
    let indent = &indent[..indent.len() - indent.trim_start().len()];

    text.split('\n')
        .enumerate()
        .map(|(i, l)| {
            let l = l.strip_suffix('\r').unwrap_or(l);

            if i == 0 {
                l
            } else {
                l.strip_prefix(indent).unwrap_or(l)
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// A signature on one line: each line trimmed and joined with a space,
/// except right inside brackets, and without a trailing comma before a
/// closing bracket.
pub fn one_line(signature: &str) -> String {
    let mut out = String::new();

    for line in signature.lines().map(str::trim).filter(|l| !l.is_empty()) {
        if line.starts_with([')', ']']) && out.ends_with(',') {
            out.pop();
        }

        if !out.is_empty() && !out.ends_with(['(', '[']) && !line.starts_with([')', ']']) {
            out.push(' ');
        }

        out.push_str(line);
    }

    out
}

/// Whether a module's source parses.
pub fn parses(text: &str) -> bool {
    ast::parser::Parser::new(text.as_bytes().to_vec(), PathBuf::from("check.inko"))
        .parse()
        .is_ok()
}

/// `text` with `lines` (0-based) blank: empty, apart from their `\n`.
pub fn blank(text: &str, lines: &BTreeSet<usize>) -> String {
    text.split_inclusive('\n')
        .enumerate()
        .map(|(i, l)| {
            if !lines.contains(&i) {
                l
            } else if l.ends_with('\n') {
                "\n"
            } else {
                ""
            }
        })
        .collect()
}

/// The depth of braces at the start of each line: how many `{` before it
/// are open. Strings and comments are skipped.
fn depths(text: &str) -> Vec<i32> {
    let mut out = Vec::new();
    let mut depth = 0;
    let mut quote = None;

    for line in text.split('\n') {
        let mut chars = line.chars();

        out.push(depth);

        while let Some(c) = chars.next() {
            match (quote, c) {
                (Some(_), '\\') => {
                    chars.next();
                }
                (Some(q), c) if c == q => quote = None,
                (Some(_), _) => {}
                (None, '"' | '\'') => quote = Some(c),
                (None, '#') => break,
                (None, '{') => depth += 1,
                (None, '}') => depth -= 1,
                _ => {}
            }
        }
    }

    out
}

/// The line (0-based) of the bracket nothing closes: the one a closing
/// bracket of another kind meets, or the innermost left open at the end.
/// The parser reports these where it notices, which may be lines later.
/// Strings and comments are skipped.
pub fn unclosed(text: &str) -> Option<usize> {
    let mut open: Vec<(char, usize)> = Vec::new();
    let mut quote = None;

    for (number, line) in text.lines().enumerate() {
        let mut chars = line.chars();

        while let Some(c) = chars.next() {
            match (quote, c) {
                (Some(_), '\\') => {
                    chars.next();
                }
                (Some(q), c) if c == q => quote = None,
                (Some(_), _) => {}
                (None, '"' | '\'') => quote = Some(c),
                (None, '#') => break,
                (None, '(' | '[' | '{') => open.push((c, number)),
                (None, ')' | ']' | '}') => {
                    let expected = match c {
                        ')' => '(',
                        ']' => '[',
                        _ => '{',
                    };

                    match open.pop() {
                        Some((o, _)) if o == expected => {}
                        Some((_, line)) => return Some(line),
                        None => return Some(number),
                    }
                }
                _ => {}
            }
        }
    }

    open.last().map(|(_, line)| *line)
}

/// The line (0-based) where parsing a module's source stops, or `None` if
/// it parses.
pub fn parse_error_line(text: &str) -> Option<usize> {
    ast::parser::Parser::new(text.as_bytes().to_vec(), PathBuf::from("check.inko"))
        .parse()
        .err()
        .map(|e| e.location.line_start.saturating_sub(1) as usize)
}

/// What a folding range folds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FoldKind {
    /// What's between a bracket and the one that closes it, on lines of
    /// its own: a type's or method's body, a block, a multi-line argument
    /// list or array.
    Block,
    /// Comment lines one after another: a doc comment, or any other.
    Comment,
    /// The imports at the top of a module, one after another.
    Imports,
}

/// A range of lines that can be folded, by byte offsets: from the line
/// `start` is on to the line `end` is on, or for a `Block`, the line
/// before it (so its closing bracket stays in sight).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fold {
    pub kind: FoldKind,
    pub start: usize,
    pub end: usize,
}

/// Where `text` can be folded, from its tokens alone, so also where it
/// doesn't parse: brackets are matched as far as they match (one closed
/// by another kind closes nothing), and a bracket nothing closes folds
/// nothing. In order of `start`, then of the longer first.
pub fn folds(text: &str) -> Vec<Fold> {
    use ast::lexer::{Lexer, TokenKind as T};

    let lines = Lines::new(text);
    let at = |line: u32, column: u32| lines.offset(line, column).unwrap_or(text.len());
    let mut lexer = Lexer::new(text.as_bytes().to_vec());
    let mut out = Vec::new();
    let mut open: Vec<(T, u32, usize)> = Vec::new();
    // The comment lines so far: the first's line and start, the last's.
    let mut comments: Option<(u32, usize, u32, usize)> = None;
    // The imports so far: the first's start, and the last token's.
    let mut imports: Option<(usize, usize)> = None;
    let mut line = 0;

    loop {
        let token = lexer.next_token();
        let l = &token.location;

        match token.kind {
            T::Null => break,
            T::Whitespace => continue,
            _ => {}
        }

        let first = l.line_start != line;
        // Only what's folded needs its offset, which counts the line's
        // characters up to it.
        let offset = || at(l.line_start, l.column_start);

        line = l.line_end;

        if token.kind == T::Comment && first {
            comments = match comments {
                Some((from, start, last, _)) if l.line_start == last + 1 => {
                    Some((from, start, l.line_start, offset()))
                }
                _ => {
                    out.extend(comment_fold(comments));

                    let offset = offset();

                    Some((l.line_start, offset, l.line_start, offset))
                }
            };
            continue;
        }

        if token.kind != T::Comment {
            out.extend(comment_fold(comments.take()));
        }

        if open.is_empty() && first && token.kind != T::Comment {
            imports = match (token.kind, imports) {
                (T::Import, None) => Some((offset(), 0)),
                (T::Import, Some(run)) => Some(run),
                (_, run) => {
                    out.extend(import_fold(text, run));
                    None
                }
            };
        }

        if let Some((_, last)) = &mut imports
            && token.kind != T::Comment
        {
            *last = offset();
        }

        match token.kind {
            T::CurlyOpen | T::ParenOpen | T::BracketOpen => {
                open.push((token.kind, l.line_start, offset()));
            }
            T::CurlyClose | T::ParenClose | T::BracketClose => {
                let opening = match token.kind {
                    T::CurlyClose => T::CurlyOpen,
                    T::ParenClose => T::ParenOpen,
                    _ => T::BracketOpen,
                };

                if let Some(i) = open.iter().rposition(|(k, _, _)| *k == opening) {
                    let (_, from, start) = open[i];

                    open.truncate(i);

                    if l.line_start >= from + 2 {
                        out.push(Fold {
                            kind: FoldKind::Block,
                            start,
                            end: offset(),
                        });
                    }
                }
            }
            _ => {}
        }
    }

    out.extend(comment_fold(comments));
    out.extend(import_fold(text, imports));
    out.sort_by_key(|f| (f.start, std::cmp::Reverse(f.end)));
    out.dedup();
    out
}

fn comment_fold(run: Option<(u32, usize, u32, usize)>) -> Option<Fold> {
    let (from, start, last, end) = run?;

    (last > from).then_some(Fold {
        kind: FoldKind::Comment,
        start,
        end,
    })
}

fn import_fold(text: &str, run: Option<(usize, usize)>) -> Option<Fold> {
    let (start, end) = run?;

    text[start..end].contains('\n').then_some(Fold {
        kind: FoldKind::Imports,
        start,
        end,
    })
}

/// `text` as `inko fmt` formats it: the upstream formatter itself
/// (compiler/src/format.rs), given the module parsed with its comments, as
/// `inko fmt` parses it. `None` if it doesn't parse, which `inko fmt`
/// refuses too.
pub fn format(text: &str) -> Option<String> {
    let module =
        ast::parser::Parser::with_comments(text.as_bytes().to_vec(), PathBuf::from("format.inko"))
            .parse()
            .ok()?;

    Some(compiler::format::Document::new().format(module))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fold_lines(text: &str) -> Vec<(FoldKind, usize, usize)> {
        let line = |offset: usize| text[..offset].matches('\n').count();

        folds(text)
            .iter()
            .map(|f| (f.kind, line(f.start), line(f.end)))
            .collect()
    }

    #[test]
    fn a_comment_after_code_is_no_comment_line() {
        let text = "let A = 1 # One.\n# Two.\nlet B = 2\n";

        assert_eq!(fold_lines(text), []);
    }

    #[test]
    fn one_line_folds_nothing() {
        let text = "# One.\nlet A = [\n  1]\nlet B = 2\n";

        assert_eq!(fold_lines(text), []);
    }

    #[test]
    fn comment_lines_in_a_body_fold() {
        let text = "type A {\n  # One.\n  #\n  # Two.\n  let @a: Int\n}\n";

        assert_eq!(
            fold_lines(text),
            [(FoldKind::Block, 0, 5), (FoldKind::Comment, 1, 3)]
        );
    }

    const TEXT: &str = "\
# The module.

import std.stdio (Stdout, STDIN as IN)
import std.fmt

# A counter.
# Counts.
type pub Counter[T] {
  let @count: Int
  let pub @label: String

  fn pub static new(
    label: String,
    count: Int,
  ) -> Counter[T] {
    Counter(count: count, label: label)
  }

  fn add(amount: Int) -> Int {
    @count + amount
  }
}

type pub enum Shape {
  case Circle(Float)
  case Square
}

trait Area {
  fn area -> Float

  fn pub double -> Float {
    area * 2.0
  }
}

impl Area for Shape {
  fn area -> Float {
    1.0
  }
}

impl Counter {
  fn pub +(other: ref Counter) -> Counter {
    Counter(count: 0, label: \"\")
  }
}

let pub MAX = 10

fn pub main {}
";

    /// Each declaration as `kind name: signature`, indented by depth.
    fn shown(outline: &Outline, text: &str) -> Vec<String> {
        outline
            .all()
            .into_iter()
            .map(|(d, parents)| {
                assert_eq!(
                    &text[d.name_range.clone()].trim_start_matches('@'),
                    &d.name.as_str().split(' ').next_back().unwrap(),
                    "{:?}",
                    d
                );
                assert!(text[d.range.clone()].starts_with(d.signature.lines().next().unwrap()));

                format!(
                    "{}{} {}{}: {}",
                    "  ".repeat(parents.len()),
                    d.kind.name(),
                    if d.public { "pub " } else { "" },
                    d.name,
                    d.detail()
                )
            })
            .collect()
    }

    #[test]
    fn declarations_with_their_signatures_as_written() {
        let outline = parse(TEXT).unwrap();

        assert_eq!(
            shown(&outline, TEXT),
            [
                "type pub Counter: type pub Counter[T]",
                "  field count: let @count: Int",
                "  field pub label: let pub @label: String",
                "  static method pub new: fn pub static new(label: String, count: Int) -> Counter[T]",
                "  method add: fn add(amount: Int) -> Int",
                "enum pub Shape: type pub enum Shape",
                "  constructor pub Circle: case Circle(Float)",
                "  constructor pub Square: case Square",
                "trait Area: trait Area",
                "  method area: fn area -> Float",
                "  method pub double: fn pub double -> Float",
                "impl pub impl Area for Shape: impl Area for Shape",
                "  method area: fn area -> Float",
                "impl pub impl Counter: impl Counter",
                "  method pub +: fn pub +(other: ref Counter) -> Counter",
                "constant pub MAX: let pub MAX = 10",
                "module method pub main: fn pub main",
            ]
        );

        // As written, with the first line's indentation off the others.
        let new = &outline.declarations[0].children[2];

        assert_eq!(
            new.signature,
            "fn pub static new(\n  label: String,\n  count: Int,\n) -> Counter[T]"
        );
        assert_eq!(&TEXT[new.name_range.clone()], "new");
        assert!(TEXT[new.range.clone()].ends_with("label: label)\n  }"));
        // A field's name is located with its `@`.
        assert_eq!(
            &TEXT[outline.declarations[0].children[0].name_range.clone()],
            "@count"
        );
    }

    #[test]
    fn imports_and_doc_comments() {
        let outline = parse(TEXT).unwrap();
        let stdio = &outline.imports[0];

        assert_eq!(stdio.path, ["std", "stdio"]);
        assert_eq!(stdio.binds(), ["Stdout", "IN"]);
        assert_eq!(&TEXT[stdio.list.clone().unwrap()], "(Stdout, STDIN as IN)");
        assert_eq!(outline.imports[1].binds(), ["fmt"]);
        // The type's doc comment, the lines above it.
        assert_eq!(outline.documentation.as_deref(), Some("The module."));
        assert_eq!(
            outline.declarations[0].documentation.as_deref(),
            Some("A counter.\nCounts.")
        );
        assert_eq!(outline.declarations[0].children[0].documentation, None);
        // What a hover reads: the doc comment and the signature.
        assert_eq!(
            &TEXT[outline.declarations[0].header.clone()],
            "# A counter.\n# Counts.\ntype pub Counter[T]"
        );
        assert_eq!(
            &TEXT[outline.declarations[0].children[0].header.clone()],
            "let @count: Int"
        );
        assert_eq!(outline.items[0], (None, 2));
        assert_eq!(outline.items[2], (Some(5), 7));
        assert_eq!(outline.items[3], (None, 23));
    }

    #[test]
    fn text_that_doesnt_parse_is_read_with_lines_blank() {
        let broken = TEXT.replace("    @count + amount", "    @count + ");
        let read = outline(&broken).unwrap();

        assert!(parse(&broken).is_none());
        // The line the parser stops at is the `}` after it.
        assert_eq!(read.blank, [19]);
        assert_eq!(shown(&read, &broken), shown(&parse(TEXT).unwrap(), TEXT));

        // A signature with a line in the way: not as the source writes it,
        // so the method isn't there.
        let broken = TEXT.replace(
            "    label: String,\n    count: Int,",
            "    label: String,\n    count: ,",
        );
        let read = outline(&broken).unwrap();
        let names: Vec<&str> = read.all().iter().map(|(d, _)| d.name.as_str()).collect();

        assert_eq!(read.blank, [13]);
        assert!(!names.contains(&"new"));
        assert!(names.contains(&"add"));

        // A type's first line in the way: what was in it may be what
        // follows it at the top of the copy, so that's left out. What's
        // before it is there.
        let broken = TEXT.replace("type pub enum Shape {", "type pub enum Shape");
        let read = outline(&broken).unwrap();
        let names: Vec<&str> = read.all().iter().map(|(d, _)| d.name.as_str()).collect();

        assert!(!read.blank.is_empty());
        assert_eq!(names, ["Counter", "count", "label", "new", "add"]);

        // One with its `{` but not its `}`: the type isn't nested as the
        // text has it, nor is anything after it.
        let broken = TEXT.replace("type pub Counter[T] {", "type pub Counter[T] {\n  {");
        let read = outline(&broken).unwrap();

        assert!(read.all().iter().all(|(d, _)| d.name != "Counter"));

        // An unfinished signature: the method is gone, the rest is there.
        let broken = TEXT.replace("  fn add(amount: Int) -> Int {", "  fn add(amount: ");
        let read = outline(&broken).unwrap();
        let names: Vec<String> = read.all().iter().map(|(d, _)| d.name.clone()).collect();

        assert!(!names.contains(&"add".to_string()));
        assert!(names.contains(&"main".to_string()));
        assert!(names.contains(&"Shape".to_string()));
    }

    #[test]
    fn a_blank_line_that_opened_a_brace_moves_what_follows() {
        // In the text, `b` is in `a`, whose `if` isn't closed, and `a`
        // ends at the `}` that closes the `if`: a copy with the `if`
        // blank nests neither as the text does.
        let text = "fn a {\n  if x {\n  1\n}\n\nfn b {}\n";
        let lines = BTreeSet::from([1]);
        let copy = blank(text, &lines);
        let read = parse(&copy).unwrap();

        assert_eq!(read.declarations.len(), 2);

        let read = read.moved(&copy, text).nested_as_in(text, &copy, &lines);

        assert!(read.declarations.is_empty(), "{:?}", read.declarations);

        // A balanced blank line moves nothing.
        let text = "fn a {\n  if x { 1 } else {}\n}\n\nfn b {}\n";
        let copy = blank(text, &lines);
        let read = parse(&copy)
            .unwrap()
            .moved(&copy, text)
            .nested_as_in(text, &copy, &lines);

        assert_eq!(read.declarations.len(), 2);
    }

    #[test]
    fn doc_comments_are_the_comments_right_above() {
        let docs = |text: &str| {
            parse(text)
                .unwrap()
                .declarations
                .iter()
                .map(|d| d.documentation.clone())
                .collect::<Vec<_>>()
        };

        // A blank line between comments starts them over; one between a
        // comment and a declaration leaves it none.
        assert_eq!(
            docs("fn a {}\n# Not this.\n\n# This.\nfn b {}\n"),
            [None, Some("This.".to_string())]
        );
        assert_eq!(docs("fn a {}\n# Not this.\n\nfn b {}\n"), [None, None]);
    }

    #[test]
    fn imports_being_typed() {
        let module = |s: &[&str]| {
            Some(ImportContext::Module(
                s.iter().map(|s| s.to_string()).collect(),
            ))
        };

        assert_eq!(import_context("import "), module(&[]));
        assert_eq!(import_context("fn a {}\nimport std.fm"), module(&["std"]));
        assert_eq!(import_context("import std.net.ht"), module(&["std", "net"]));
        assert_eq!(
            import_context("import std.stdio (Stdout, STDIN as IN, St"),
            Some(ImportContext::Symbol(
                "std.stdio".to_string(),
                vec!["Stdout".to_string(), "STDIN".to_string()]
            ))
        );
        assert_eq!(
            import_context("import std.stdio (\n  Stdout,\n  "),
            Some(ImportContext::Symbol(
                "std.stdio".to_string(),
                vec!["Stdout".to_string()]
            ))
        );
        assert_eq!(import_context("import std.stdio (Stdout)\nfn a"), None);
        assert_eq!(import_context("import std.stdio\nfn a"), None);
        assert_eq!(import_context("import extern \"m"), None);
        assert_eq!(import_context("  let import = 1\n  im"), None);
        assert_eq!(import_context("import std.stdio if li"), None);
    }

    #[test]
    fn imports_are_added_as_inko_fmt_orders_them() {
        let edit = |text: &str, module: &str, symbol: &str| {
            let (range, with) = import_edit(text, &parse(text).unwrap(), module, symbol);
            let mut out = text.to_string();

            out.replace_range(range, &with);
            out
        };
        let main = "fn main {}\n";

        // Into an existing list, in order, `self` first.
        assert_eq!(
            edit(
                "import std.stdio (self, STDIN, Stdout)\n\n",
                "std.stdio",
                "Stderr"
            ),
            "import std.stdio (self, STDIN, Stderr, Stdout)\n\n"
        );
        assert_eq!(
            edit("import std.stdio (Stdin)\n", "std.stdio", "Stdout"),
            "import std.stdio (Stdin, Stdout)\n"
        );
        assert_eq!(
            edit("import std.stdio (Stdout)\n", "std.stdio", "Stdin"),
            "import std.stdio (Stdin, Stdout)\n"
        );
        // An import of the module alone keeps it.
        assert_eq!(
            edit("import std.stdio\n", "std.stdio", "Stdout"),
            "import std.stdio (self, Stdout)\n"
        );
        // A new import, in the first group, by path.
        assert_eq!(
            edit(
                "import std.fmt\nimport std.string (StringBuffer)\n\nfn main {}\n",
                "std.stdio",
                "Stdout"
            ),
            "import std.fmt\nimport std.stdio (Stdout)\nimport std.string (StringBuffer)\n\nfn main {}\n"
        );
        assert_eq!(
            edit("import std.fmt\n\nfn main {}\n", "std.stdio", "Stdout"),
            "import std.fmt\nimport std.stdio (Stdout)\n\nfn main {}\n"
        );
        assert_eq!(
            edit("import std.stdio if linux\n", "std.stdio", "Stdout"),
            "import std.stdio if linux\nimport std.stdio (Stdout)\n"
        );
        // None yet: before the first declaration and its doc comment.
        assert_eq!(
            edit(main, "std.stdio", "Stdout"),
            "import std.stdio (Stdout)\n\nfn main {}\n"
        );
        assert_eq!(
            edit(
                "# The module.\n\n# Main.\nfn main {}\n",
                "std.stdio",
                "Stdout"
            ),
            "# The module.\n\nimport std.stdio (Stdout)\n\n# Main.\nfn main {}\n"
        );
        assert_eq!(
            edit("", "std.stdio", "Stdout"),
            "import std.stdio (Stdout)\n"
        );
    }

    #[test]
    fn an_impl_blocks_owner_is_its_type() {
        let owner = |name: &str| {
            Declaration {
                kind: Kind::Implementation,
                name: name.to_string(),
                name_range: 0..0,
                range: 0..0,
                header: 0..0,
                signature: String::new(),
                public: true,
                documentation: None,
                children: Vec::new(),
            }
            .owner()
            .to_string()
        };

        assert_eq!(owner("impl Counter"), "Counter");
        assert_eq!(owner("impl Area for Shape"), "Shape");
        assert_eq!(owner("impl Equal[T] for Array[T] if T: Equal"), "Array");
        assert_eq!(owner("impl Counter if T: Equal"), "Counter");
    }

    #[test]
    fn signatures_on_one_line() {
        assert_eq!(
            one_line("fn a(\n  b: Int,\n  c: Int,\n) -> Int"),
            "fn a(b: Int, c: Int) -> Int"
        );
        assert_eq!(one_line("let A = [\n  1,\n  2,\n]"), "let A = [1, 2]");
        assert_eq!(
            one_line("impl Equal for Array\nif\n  T: Equal"),
            "impl Equal for Array if T: Equal"
        );
    }
}
