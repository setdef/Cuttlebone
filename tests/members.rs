//! membersAt, driven the way Parakeet completes `receiver.|`: a copy of the
//! buffer with a placeholder name at the caret, sent as unsaved contents.
mod common;

use common::{PLACEHOLDER, Project, position_of};
use cuttlebone::query::{self, MemberKind, MembersAt};

const MAIN: &str = "src/main.inko";

fn members(before: &str, after: &str) -> MembersAt {
    Project::new("members")
        .members_at(MAIN, before, after)
        .unwrap()
        .unwrap_or_else(|| panic!("no members at {:?}", after))
}

fn names(m: &MembersAt) -> Vec<&str> {
    m.members.iter().map(|m| m.name.as_str()).collect()
}

fn member<'a>(m: &'a MembersAt, name: &str) -> &'a query::Member {
    m.members
        .iter()
        .find(|m| m.name == name)
        .unwrap_or_else(|| panic!("no member {:?} in {:?}", name, names(m)))
}

fn placeholder(receiver: &str) -> String {
    format!("{}.{}", receiver, PLACEHOLDER)
}

#[test]
fn the_receiver_keeps_its_type_when_followed_by_the_placeholder() {
    let p = Project::new("members");
    let text = p.read(MAIN).replacen("name.size", &placeholder("name"), 1);
    let t = p
        .type_at(MAIN, Some(&text), &placeholder("name"), 0)
        .unwrap()
        .unwrap();

    assert_eq!(t.typ, "String");
}

#[test]
fn a_standard_library_type_lists_its_instance_methods_with_signatures() {
    let m = members("name.size", &placeholder("name"));
    let size = member(&m, "size");

    assert_eq!(m.receiver, "String");
    assert_eq!(size.kind, MemberKind::Method);
    assert_eq!(size.signature, "fn pub inline size -> Int");
    assert_eq!(size.typ, "Int");
    assert_eq!(size.module, "std.string");
    assert!(names(&m).contains(&"to_upper"));
    // Sorted by name, without duplicates.
    let mut sorted = names(&m);

    sorted.sort();
    sorted.dedup();
    assert_eq!(names(&m), sorted);
}

#[test]
fn the_receiver_range_is_the_receiver() {
    let p = Project::new("members");
    let text = p.read(MAIN).replacen("name.size", &placeholder("name"), 1);
    let m = members("name.size", &placeholder("name"));
    let start = position_of(&text, &placeholder("name"), 0);

    assert_eq!(m.receiver_range.start, start);
    assert_eq!(m.receiver_range.end.character, start.character + 4);
}

#[test]
fn type_arguments_are_filled_in() {
    let m = members("numbers.get(0)", &format!("{}(0)", placeholder("numbers")));

    assert_eq!(m.receiver, "Array[Int]");
    assert_eq!(
        member(&m, "get").signature,
        "fn pub inline get(index: Int) -> Result[ref Int, OutOfBounds]"
    );
    // Including in methods from `impl Array if T: Equal`, which use a copy of
    // `T` with that bound.
    assert_eq!(
        member(&m, "contains?").signature,
        "fn pub contains?(value: ref Int) -> Bool"
    );
}

#[test]
fn an_owned_receiver_of_a_type_in_the_same_module_sees_private_and_mutating_methods() {
    let m = members("counter.count", &placeholder("counter"));

    assert_eq!(m.receiver, "Counter");

    for name in ["count", "bump", "helper", "label"] {
        assert!(
            names(&m).contains(&name),
            "{} missing: {:?}",
            name,
            names(&m)
        );
    }

    let label = member(&m, "label");

    assert_eq!(label.kind, MemberKind::Field);
    assert_eq!(label.signature, "let pub @label: String");
    assert_eq!(label.typ, "String");
    // A static method can't be called on an instance.
    assert!(!names(&m).contains(&"new"));
}

#[test]
fn a_ref_receiver_doesnt_see_methods_that_need_a_mutable_receiver() {
    // `self` is `ref Counter` in `fn helper`, which isn't `fn mut`.
    let m = members("self.count", &placeholder("self"));

    assert_eq!(m.receiver, "ref Counter");
    assert!(names(&m).contains(&"count"));
    assert!(!names(&m).contains(&"bump"), "{:?}", names(&m));
}

#[test]
fn a_type_as_receiver_lists_its_static_methods() {
    let m = members(
        "Counter.new(name)",
        &format!("{}(name)", placeholder("Counter")),
    );

    assert_eq!(m.receiver, "Counter");
    assert_eq!(names(&m), vec!["new"]);
    assert_eq!(
        member(&m, "new").signature,
        "fn static new(label: String) -> Counter"
    );
}

#[test]
fn private_members_of_another_root_namespace_are_hidden() {
    let m = members("circle.area", &placeholder("circle"));

    assert_eq!(m.receiver, "Circle");
    assert!(names(&m).contains(&"area"));
    assert!(names(&m).contains(&"radius"));
    assert!(!names(&m).contains(&"hidden"), "{:?}", names(&m));
    assert!(!names(&m).contains(&"cache"), "{:?}", names(&m));
}

#[test]
fn a_module_lists_its_public_methods_constants_and_types() {
    let m = members("shapes.PI", &placeholder("shapes"));

    assert_eq!(m.receiver, "shapes");
    assert_eq!(member(&m, "PI").kind, MemberKind::Constant);
    assert_eq!(member(&m, "PI").signature, "let pub PI: Float");
    assert_eq!(member(&m, "Circle").kind, MemberKind::Type);
    assert_eq!(member(&m, "unit").kind, MemberKind::Method);
    assert!(!names(&m).contains(&"SECRET"));
    assert!(!names(&m).contains(&"private_helper"));
}

#[test]
fn only_self_sees_an_async_types_private_methods() {
    let from_self = members("self.work", &placeholder("self"));
    let from_outside = members("worker.go", &placeholder("worker"));

    assert_eq!(names(&from_self), vec!["go", "work"]);
    assert_eq!(names(&from_outside), vec!["go"]);
}

#[test]
fn a_type_parameter_lists_its_requirements_methods() {
    let m = members("value.to_string", &placeholder("value"));

    assert_eq!(m.receiver, "ref T");
    assert_eq!(names(&m), vec!["to_string"]);
}

#[test]
fn a_prefix_before_the_placeholder_still_finds_the_call() {
    // Completing `name.si|` inserts the placeholder after the prefix.
    let m = members("name.size", &format!("name.si{}", PLACEHOLDER));

    assert_eq!(m.receiver, "String");
}

#[test]
fn the_position_right_after_the_name_also_finds_the_call() {
    let p = Project::new("members");
    let after = format!("{}(name)", placeholder("Counter"));
    let text = p.read(MAIN).replacen("Counter.new(name)", &after, 1);
    let end = placeholder("Counter").len() as u32;
    let mut request = p.request(MAIN, Some(&text), &after, end);

    assert_eq!(
        query::members_at(&p.session, &request)
            .result
            .unwrap()
            .unwrap()
            .receiver,
        "Counter"
    );

    // But not one further, inside the argument list.
    request.position.character += 1;
    assert_eq!(query::members_at(&p.session, &request).result, Ok(None));
}

#[test]
fn an_untyped_receiver_is_unavailable() {
    let p = Project::new("members");
    let err = p
        .members_at(MAIN, "name.size", &placeholder("missing"))
        .unwrap_err();

    assert_eq!(err.code, "untyped");
}

#[test]
fn a_position_that_isnt_a_call_has_no_members() {
    let p = Project::new("members");
    let request = p.request(MAIN, None, "let name", 0);

    assert_eq!(query::members_at(&p.session, &request).result, Ok(None));
}

#[test]
fn a_generic_traits_default_methods_get_the_types_arguments() {
    // IntoIter[Int] implements Iter[T] with its own T, and gets Iter's
    // default methods, which use Iter's T.
    let m = members(
        "numbers.get(0).or(0)",
        &format!("numbers.into_iter.{}", PLACEHOLDER),
    );

    assert_eq!(m.receiver, "IntoIter[Int]");
    assert_eq!(
        member(&m, "to_array").signature,
        "fn pub move to_array -> Array[Int]"
    );
    assert_eq!(
        member(&m, "map").signature,
        "fn pub move map[R](func: fn (Int) -> R) -> Map[Int, IntoIter[Int], R]"
    );
}
