//! Which answers from a check of other text (an older version of the
//! documents, or a repaired copy) still hold for the documents as they are.
//!
//! A file is split into units: each method, and each other item (a type, a
//! trait, an `impl` block, a constant, an import) apart from its methods. A
//! method's types depend on its own text and on the declarations of what it
//! uses, never on another method's body. So a change can only make a unit
//! mean something else if:
//!
//! - it's in the unit, or
//! - it isn't inside a method's body (it's in a signature, a field, an
//!   import, a type's header, or between items), and the unit mentions one
//!   of the names the change is about: the method, field, type or other
//!   item it's in, the names in the text it removed or added, or for a
//!   change between items, the names in that text. A unit mentions a name
//!   if its text has it, or the type of one of its expressions does.
//!
//! Every other unit means what it did in the check, so its answers hold.
//! Comments and blank lines between items change nothing.
//!
//! An error the type checker reports outside a method's body (in a
//! declaration) is treated like a change there (`error_effect`): the types
//! the check gave everything that uses the declaration are those of a
//! declaration that will be different once it's fixed.
use crate::edits::Change;
use crate::position::Lines;
use location::Location;
use std::collections::HashSet;
use std::ops::Range;
use std::path::PathBuf;

/// What kind of item an `Item` is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Method,
    /// A type, trait or `impl` block: a container of methods.
    Container,
    Field,
    Constructor,
    Constant,
    Import,
    Comment,
}

/// An item of a file, in bytes.
#[derive(Clone, Debug)]
pub struct Item {
    pub kind: Kind,
    pub range: Range<usize>,
    /// A method's body, braces included.
    pub body: Option<Range<usize>>,
    /// Where its name is.
    pub name: Option<Range<usize>>,
    /// The names it declares, or for an `impl` block the trait and the
    /// type.
    pub names: Vec<String>,
    /// The item it's in.
    pub parent: Option<usize>,
}

/// A parsed file's items.
pub struct Outline {
    pub items: Vec<Item>,
}

/// A unit: a method, an item other than a method (without its methods), or
/// what's between items.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Unit {
    Item(usize),
    Between,
}

impl Outline {
    /// The outline of `text`, or `None` if it doesn't parse.
    pub fn parse(text: &str) -> Option<Outline> {
        let module = ast::parser::Parser::new(text.as_bytes().to_vec(), PathBuf::from("outline"))
            .parse()
            .ok()?;
        let mut outline = Builder {
            lines: Lines::new(text),
            items: Vec::new(),
        };

        for e in &module.expressions {
            outline.top(e);
        }

        Some(Outline {
            items: outline.items,
        })
    }

    /// The unit the byte at `offset` is in: the innermost method containing
    /// it, or else the innermost other item that isn't a comment, field or
    /// constructor (those are their type's).
    pub fn unit_at(&self, offset: usize) -> Unit {
        let inside = |kinds: &[Kind]| {
            self.items
                .iter()
                .enumerate()
                .filter(|(_, i)| kinds.contains(&i.kind) && i.range.contains(&offset))
                .max_by_key(|(_, i)| i.range.start)
                .map(|(index, _)| Unit::Item(index))
        };

        inside(&[Kind::Method])
            .or_else(|| inside(&[Kind::Container, Kind::Constant, Kind::Import]))
            .unwrap_or(Unit::Between)
    }

    /// Whether an item is a unit of its own: a method, or an item other
    /// than a comment, field or constructor (which are their type's).
    pub fn unit_at_item(&self, unit: Unit) -> bool {
        match unit {
            Unit::Item(i) => matches!(
                self.items[i].kind,
                Kind::Method | Kind::Container | Kind::Constant | Kind::Import
            ),
            Unit::Between => true,
        }
    }

    /// A unit's own bytes: a method's or item's range, without the methods
    /// in it. `Between` has none.
    pub fn own(&self, unit: Unit) -> Vec<Range<usize>> {
        let Unit::Item(index) = unit else {
            return Vec::new();
        };
        let item = &self.items[index];
        let mut out = Vec::new();
        let mut at = item.range.start;
        let mut methods: Vec<&Item> = self
            .items
            .iter()
            .filter(|i| i.parent == Some(index) && i.kind == Kind::Method)
            .collect();

        methods.sort_by_key(|m| m.range.start);

        for m in methods {
            out.push(at..m.range.start);
            at = m.range.end;
        }

        out.push(at..item.range.end);
        out.retain(|r| !r.is_empty());
        out
    }

    /// The innermost item containing all of `range`. An empty range (an
    /// insertion) at an item's very start or end is outside it.
    fn innermost(&self, range: &Range<usize>) -> Option<usize> {
        self.items
            .iter()
            .enumerate()
            .filter(|(_, i)| {
                i.range.start <= range.start
                    && range.end <= i.range.end
                    && !(range.is_empty()
                        && (range.start == i.range.start || range.start == i.range.end))
            })
            .max_by_key(|(_, i)| (i.range.start, std::cmp::Reverse(i.range.end)))
            .map(|(index, _)| index)
    }

    /// An item, and the items it's in.
    fn ancestors(&self, index: usize) -> Vec<&Item> {
        let mut out = vec![&self.items[index]];

        while let Some(parent) = out.last().unwrap().parent {
            out.push(&self.items[parent]);
        }

        out
    }
}

struct Builder<'a> {
    lines: Lines<'a>,
    items: Vec<Item>,
}

impl Builder<'_> {
    fn bytes(&self, l: &Location) -> Range<usize> {
        let at = |line: u32, column: u32| self.lines.offset(line, column).unwrap_or(0);

        at(l.line_start, l.column_start)..at(l.line_end, l.column_end + 1)
    }

    fn push(
        &mut self,
        kind: Kind,
        location: &Location,
        name: Option<(&str, &Location)>,
        parent: Option<usize>,
    ) -> usize {
        let item = Item {
            kind,
            range: self.bytes(location),
            body: None,
            name: name.map(|(_, l)| self.bytes(l)),
            names: name.map(|(n, _)| vec![n.to_string()]).unwrap_or_default(),
            parent,
        };

        self.items.push(item);
        self.items.len() - 1
    }

    fn method(&mut self, m: &ast::nodes::DefineMethod, parent: Option<usize>) {
        let index = self.push(
            Kind::Method,
            &m.location,
            Some((&m.name.name, &m.name.location)),
            parent,
        );

        self.items[index].body = m.body.as_ref().map(|b| self.bytes(&b.location));
    }

    fn top(&mut self, e: &ast::nodes::TopLevelExpression) {
        use ast::nodes::{
            ImplementationExpression as I, TopLevelExpression as T, TraitExpression as R,
            TypeExpression as Y,
        };

        match e {
            T::DefineMethod(m) => self.method(m, None),
            T::DefineType(t) => {
                let index = self.push(
                    Kind::Container,
                    &t.location,
                    Some((&t.name.name, &t.name.location)),
                    None,
                );

                for e in &t.body.values {
                    match e {
                        Y::DefineMethod(m) => self.method(m, Some(index)),
                        Y::DefineField(f) => {
                            self.push(
                                Kind::Field,
                                &f.location,
                                Some((&f.name.name, &f.name.location)),
                                Some(index),
                            );
                        }
                        Y::DefineConstructor(c) => {
                            self.push(
                                Kind::Constructor,
                                &c.location,
                                Some((&c.name.name, &c.name.location)),
                                Some(index),
                            );
                        }
                        Y::Comment(c) => {
                            self.push(Kind::Comment, &c.location, None, Some(index));
                        }
                    }
                }
            }
            T::DefineTrait(t) => {
                let index = self.push(
                    Kind::Container,
                    &t.location,
                    Some((&t.name.name, &t.name.location)),
                    None,
                );

                for e in &t.body.values {
                    match e {
                        R::DefineMethod(m) => self.method(m, Some(index)),
                        R::Comment(c) => {
                            self.push(Kind::Comment, &c.location, None, Some(index));
                        }
                    }
                }
            }
            T::ReopenType(t) => {
                let index = self.push(
                    Kind::Container,
                    &t.location,
                    Some((&t.type_name.name, &t.type_name.location)),
                    None,
                );

                self.implementation(index, &t.body.values);
            }
            T::ImplementTrait(t) => {
                let index = self.push(
                    Kind::Container,
                    &t.location,
                    Some((&t.type_name.name, &t.type_name.location)),
                    None,
                );

                self.items[index].names.push(t.trait_name.name.name.clone());

                for e in &t.body.values {
                    match e {
                        I::DefineMethod(m) => self.method(m, Some(index)),
                        I::Comment(c) => {
                            self.push(Kind::Comment, &c.location, None, Some(index));
                        }
                    }
                }
            }
            T::DefineConstant(c) => {
                self.push(
                    Kind::Constant,
                    &c.location,
                    Some((&c.name.name, &c.name.location)),
                    None,
                );
            }
            T::Import(i) => {
                self.push(Kind::Import, &i.location, None, None);
            }
            T::ExternImport(i) => {
                self.push(Kind::Import, &i.location, None, None);
            }
            T::Comment(c) => {
                self.push(Kind::Comment, &c.location, None, None);
            }
        }
    }

    fn implementation(&mut self, index: usize, body: &[ast::nodes::ImplementationExpression]) {
        for e in body {
            match e {
                ast::nodes::ImplementationExpression::DefineMethod(m) => {
                    self.method(m, Some(index))
                }
                ast::nodes::ImplementationExpression::Comment(c) => {
                    self.push(Kind::Comment, &c.location, None, Some(index));
                }
            }
        }
    }
}

/// Keywords, which every unit mentions, and which name nothing.
const KEYWORDS: &[&str] = &[
    "and", "as", "async", "break", "builtin", "case", "copy", "else", "enum", "extern", "false",
    "fn", "for", "if", "impl", "import", "inline", "let", "loop", "match", "move", "mut", "next",
    "nil", "or", "pub", "recover", "ref", "return", "self", "Self", "static", "throw", "trait",
    "true", "try", "type", "uni", "while",
];

fn is_name(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '?'
}

/// The names in `text`: identifiers and constants, without keywords.
pub fn names(text: &str) -> impl Iterator<Item = &str> {
    text.split(|c: char| !is_name(c))
        .filter(|w| w.starts_with(|c: char| c.is_alphabetic() || c == '_'))
        .filter(|w| !KEYWORDS.contains(w))
}

/// A change's old and new ranges, widened to whole names: at an end where
/// the removed or added text is part of a name, over the rest of the name.
fn widen(old: &str, new: &str, change: &Change) -> (Range<usize>, Range<usize>) {
    let (removed, added) = (&old[change.old.clone()], &new[change.new.clone()]);
    let left = removed.starts_with(is_name) || added.starts_with(is_name);
    let right = removed.ends_with(is_name) || added.ends_with(is_name);
    // The text before and after a change is the same in both.
    let before = if left {
        old[..change.old.start]
            .chars()
            .rev()
            .take_while(|c| is_name(*c))
            .map(char::len_utf8)
            .sum::<usize>()
    } else {
        0
    };
    let after = if right {
        old[change.old.end..]
            .chars()
            .take_while(|c| is_name(*c))
            .map(char::len_utf8)
            .sum::<usize>()
    } else {
        0
    };

    (
        change.old.start - before..change.old.end + after,
        change.new.start - before..change.new.end + after,
    )
}

/// The names that text declares, if it's whole items (at the top level of
/// a module, or in a type's body), such as a method typed in full or blank
/// in a repaired copy: what the items declare, with the members of types,
/// and every name in an import. `None` if it isn't, or is only blank.
fn declared(text: &str) -> Option<HashSet<String>> {
    if blank(text) {
        return None;
    }

    let wrapped = format!("type CuttleboneItems {{\n{}\n}}", text);
    let (outline, source, skip) = match Outline::parse(text) {
        Some(o) => (o, text, None),
        None => (Outline::parse(&wrapped)?, wrapped.as_str(), Some(0)),
    };
    let mut out = HashSet::new();

    for (i, item) in outline.items.iter().enumerate() {
        if Some(i) == skip {
            continue;
        }

        match item.kind {
            Kind::Import => out.extend(names(&source[item.range.clone()]).map(String::from)),
            Kind::Comment => {}
            _ => out.extend(item.names.iter().cloned()),
        }
    }

    Some(out)
}

/// Whether text is only blank or comments: each line, trimmed, is empty or
/// starts with `#`.
fn blank(text: &str) -> bool {
    text.lines().all(|l| {
        let l = l.trim();

        l.is_empty() || l.starts_with('#')
    })
}

/// What a change to a file does to the units of a check of its old text.
#[derive(Clone, Debug, Default)]
pub struct Effect {
    /// The old text it changed, widened to whole names. A unit of the same
    /// file that this touches changed itself.
    pub old: Option<Range<usize>>,
    /// Names whose meaning it may change: units that mention one may mean
    /// something else now.
    pub names: HashSet<String>,
    /// Names it added, which may be the start of a name still being typed:
    /// units that mention a name starting with one may mean something
    /// else once it's typed.
    pub prefixes: HashSet<String>,
    /// The names in the text it added, wherever it is: a list of a symbol's
    /// uses may be missing one there.
    pub added: Vec<String>,
    /// Whether it's inside a method's body.
    pub body: bool,
}

impl Effect {
    /// Whether a unit that mentions `name` may mean something else.
    pub fn affects(&self, name: &str) -> bool {
        self.names.contains(name) || self.prefixes.iter().any(|p| name.starts_with(p.as_str()))
    }
}

/// What `change` (from `old`, outlined by `outline`, to `new`) does.
pub fn effect(outline: &Outline, old: &str, new: &str, change: &Change) -> Effect {
    let (old_range, new_range) = widen(old, new, change);
    let (removed, added) = (&old[old_range.clone()], &new[new_range]);
    let innermost = outline.innermost(&old_range);
    let mut effect = Effect {
        old: Some(old_range.clone()),
        added: names(added).map(String::from).collect(),
        ..Effect::default()
    };
    let changed = |effect: &mut Effect| {
        for (text, added) in [(removed, false), (added, true)] {
            match declared(text) {
                // Whole declarations: only what they declare.
                Some(declared) => effect.names.extend(declared),
                None if added => effect.prefixes.extend(names(text).map(String::from)),
                None => effect.names.extend(names(text).map(String::from)),
            }
        }
    };
    let Some(index) = innermost else {
        // Between items: what it removed or added may declare or import
        // something.
        if !(blank(removed) && blank(added)) {
            changed(&mut effect);
        }

        return effect;
    };
    let item = &outline.items[index];

    if item.kind == Kind::Method
        && let Some(body) = &item.body
        && body.start < old_range.start
        && old_range.end < body.end
    {
        // Inside a method's body: only the method changed.
        effect.body = true;
        return effect;
    }

    if item.kind == Kind::Comment && blank(added) {
        return effect;
    }

    if item.kind == Kind::Container && blank(removed) && blank(added) {
        // Between a type's members.
        return effect;
    }

    for i in outline.ancestors(index) {
        effect.names.extend(i.names.iter().cloned());
    }

    // A method's, field's or constructor's own name, renamed; or anything
    // in another item (an import, a type's header) or between a type's
    // members (a new method).
    let named = matches!(item.kind, Kind::Method | Kind::Field | Kind::Constructor);

    if !named
        || item
            .name
            .as_ref()
            .is_some_and(|n| n.start < old_range.end.max(n.start + 1) && old_range.start < n.end)
    {
        changed(&mut effect);
    }

    effect
}

/// What an error the type checker reported at `range` of a file's text
/// does to the units of that check.
#[derive(Debug)]
pub enum ErrorEffect {
    /// It's in a method's body, which nothing else depends on.
    Body,
    /// It's in a declaration: units that mention the names it's about may
    /// get other types once it's fixed, and its own unit is touched.
    Declaration(Effect),
    /// It's in no item (or nowhere in the text), so nothing says what it's
    /// about.
    Unknown,
}

/// What an error at `range` of `text` (outlined by `outline`) does: like a
/// change of it (`effect`), with the names of the item it's in and the
/// items around that, and for an import every name in it, since any of
/// them may be what doesn't resolve.
pub fn error_effect(outline: &Outline, text: &str, range: Range<usize>) -> ErrorEffect {
    let Some(index) = outline.innermost(&range) else {
        return ErrorEffect::Unknown;
    };
    let item = &outline.items[index];

    if item.kind == Kind::Comment {
        return ErrorEffect::Unknown;
    }

    if item.kind == Kind::Method
        && let Some(body) = &item.body
        && body.start < range.start
        && range.end < body.end
    {
        return ErrorEffect::Body;
    }

    let mut effect = Effect {
        old: Some(range),
        ..Effect::default()
    };

    for i in outline.ancestors(index) {
        effect.names.extend(i.names.iter().cloned());

        if i.kind == Kind::Import {
            effect
                .names
                .extend(names(&text[i.range.clone()]).map(String::from));
        }
    }

    ErrorEffect::Declaration(effect)
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEXT: &str = "\
import std.stdio (Stdout)

# A counter.
type Counter {
  let @count: Int

  fn static new -> Counter {
    Counter(count: 0)
  }

  fn add(amount: Int) -> Int {
    @count + amount
  }
}

fn main {
  Counter.new.add(1)
}
";

    fn change(at: &str, removed: usize, added: &str) -> (String, Change) {
        let start = TEXT.find(at).unwrap();
        let new = format!("{}{}{}", &TEXT[..start], added, &TEXT[start + removed..]);

        (
            new,
            Change {
                old: start..start + removed,
                new: start..start + added.len(),
            },
        )
    }

    fn effect_of(at: &str, removed: usize, added: &str) -> Effect {
        let (new, change) = change(at, removed, added);

        effect(&Outline::parse(TEXT).unwrap(), TEXT, &new, &change)
    }

    /// The names an effect is about, exactly or as prefixes.
    fn sorted(e: &Effect) -> Vec<&str> {
        let mut out: Vec<&str> = e
            .names
            .iter()
            .chain(&e.prefixes)
            .map(String::as_str)
            .collect();

        out.sort();
        out.dedup();
        out
    }

    #[test]
    fn units() {
        let outline = Outline::parse(TEXT).unwrap();
        let unit = |at: &str| outline.unit_at(TEXT.find(at).unwrap());
        let Unit::Item(add) = unit("@count + amount") else {
            panic!("not in a method");
        };

        assert_eq!(outline.items[add].names, ["add"]);
        assert_eq!(
            &TEXT[outline.items[add].body.clone().unwrap()],
            "{\n    @count + amount\n  }"
        );
        assert_eq!(unit("fn add"), Unit::Item(add));
        assert_eq!(unit("let @count"), unit("type Counter"));
        assert_ne!(unit("let @count"), Unit::Item(add));
        assert_eq!(unit("# A counter"), Unit::Between);
        assert_eq!(unit("Stdout"), unit("import"));

        let own = outline.own(unit("type Counter"));

        assert_eq!(own.len(), 3);
        assert!(TEXT[own[0].clone()].contains("let @count: Int"));
        assert_eq!(TEXT[own[2].clone()].trim(), "}");
    }

    #[test]
    fn a_change_in_a_body_changes_only_its_method() {
        assert!(sorted(&effect_of("@count + amount", 0, "1 + ")).is_empty());
        assert!(sorted(&effect_of("Counter.new.add", 0, "let x = 1\n  ")).is_empty());
    }

    #[test]
    fn a_change_in_a_signature_changes_the_method_and_its_type() {
        let e = effect_of("Int) -> Int", 1, "");

        assert_eq!(sorted(&e), ["Counter", "add"]);
        // A rename: the old and new names too.
        assert_eq!(
            sorted(&effect_of("add(amount", 3, "plus")),
            ["Counter", "add", "plus"]
        );
    }

    #[test]
    fn changes_to_fields_imports_and_new_items() {
        assert_eq!(
            sorted(&effect_of("Int\n\n  fn static", 3, "Float")),
            ["Counter", "count"]
        );
        assert_eq!(
            sorted(&effect_of("Stdout)", 6, "Stdin")),
            ["Stdin", "Stdout"]
        );
        // A method typed between a type's members, or between items.
        assert_eq!(
            sorted(&effect_of("\n  fn add", 0, "\n  fn size -> Int {")),
            ["Counter", "Int", "size"]
        );
        assert_eq!(
            sorted(&effect_of("fn main", 0, "fn other(a: Int) {")),
            ["Int", "a", "other"]
        );
    }

    #[test]
    fn added_names_may_be_unfinished() {
        let e = effect_of("fn main", 0, "fn d\n");

        assert!(e.affects("d"));
        assert!(e.affects("describe"));
        assert!(!e.affects("main"));
        // A removed name is complete.
        let e = effect_of("Stdout)", 6, "");

        assert!(e.affects("Stdout"));
        assert!(!e.affects("StdoutWriter"));
        assert_eq!(effect_of("Counter.new.add", 0, "x.y\n  ").added, ["x", "y"]);
    }

    #[test]
    fn whole_items_change_only_what_they_declare() {
        // A method typed in full, between a type's members or items.
        let e = effect_of("\n  fn add", 0, "\n  fn size(by: Int) -> Int { by }\n");

        assert_eq!(sorted(&e), ["Counter", "size"]);
        assert!(e.prefixes.is_empty());

        let e = effect_of(
            "fn main",
            0,
            "import std.fmt (Format)\n\nfn other(a: Int) {}\n",
        );

        assert_eq!(sorted(&e), ["Format", "fmt", "other", "std"]);
        // Unfinished, it may declare anything it names.
        assert_eq!(
            sorted(&effect_of("fn main", 0, "fn other(a: Int) {\n")),
            ["Int", "a", "other"]
        );
    }

    #[test]
    fn comments_and_blank_lines_change_nothing() {
        assert!(sorted(&effect_of("# A counter.", 0, "# More.\n")).is_empty());
        assert!(sorted(&effect_of("\nfn main", 0, "\n\n")).is_empty());
        assert!(sorted(&effect_of("\n  fn add", 0, "\n  # About add.")).is_empty());
    }

    #[test]
    fn errors_in_declarations_are_about_their_items() {
        let outline = Outline::parse(TEXT).unwrap();
        let at = |needle: &str, len: usize| {
            let start = TEXT.find(needle).unwrap();

            error_effect(&outline, TEXT, start..start + len)
        };
        let names = |e: ErrorEffect| match e {
            ErrorEffect::Declaration(e) => {
                let mut out: Vec<String> = e.names.into_iter().collect();

                out.sort();
                out
            }
            other => panic!("not a declaration's error: {:?}", other),
        };

        assert!(matches!(at("amount\n", 6), ErrorEffect::Body));
        assert_eq!(names(at("Int) -> Int", 3)), ["Counter", "add"]);
        assert_eq!(names(at("Int\n\n  fn static", 3)), ["Counter", "count"]);
        assert_eq!(names(at("Stdout", 6)), ["Stdout", "std", "stdio"]);
        assert!(matches!(at("A counter", 1), ErrorEffect::Unknown));
        assert!(matches!(at("\n\nfn main", 1), ErrorEffect::Unknown));
    }

    #[test]
    fn names_are_identifiers_and_constants() {
        let found: Vec<&str> = names("let a = Foo.new(b?, @c) # fn x").collect();

        assert_eq!(found, ["a", "Foo", "new", "b?", "c", "x"]);
    }
}
