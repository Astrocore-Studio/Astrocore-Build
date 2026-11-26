//! Грамматические функции сообщений SS14 (`ContentLocalizationManager` @ 2f4a2aad):
//! `INDEFINITE`, `THE`, `SUBJECT`, `CONJUGATE-BE` и соседи.
//!
//! В SS14 аргументом служит сущность, и род берётся из её компонента `Grammar`. Здесь
//! аргумент — уже готовое название, поэтому все сущности — средний род единственного
//! числа и не имена собственные. Компонент `Grammar` переносится вместе с персонажем (3.10).

use astrocore_loc::{Loc, LocArg};

fn text(args: &[LocArg], index: usize) -> String {
    match args.get(index) {
        Some(LocArg::Text(text)) => text.clone(),
        Some(LocArg::Number(number)) => number.to_string(),
        None => String::new(),
    }
}

/// `a` или `an` по первой букве (`FormatIndefinite`).
fn indefinite(args: &[LocArg]) -> LocArg {
    let word = text(args, 0);
    let vowel = word
        .chars()
        .next()
        .is_some_and(|first| "aeiouAEIOU".contains(first));
    LocArg::Text(if vowel { "an" } else { "a" }.to_owned())
}

fn the(args: &[LocArg]) -> LocArg {
    LocArg::Text(format!("the {}", text(args, 0)))
}

fn constant(value: &'static str) -> impl Fn(&[LocArg]) -> LocArg {
    move |_| LocArg::Text(value.to_owned())
}

fn subject(_: &[LocArg]) -> LocArg {
    constant("it")(&[])
}

fn object(_: &[LocArg]) -> LocArg {
    constant("it")(&[])
}

fn poss_adj(_: &[LocArg]) -> LocArg {
    constant("its")(&[])
}

fn reflexive(_: &[LocArg]) -> LocArg {
    constant("itself")(&[])
}

fn conjugate_be(_: &[LocArg]) -> LocArg {
    constant("is")(&[])
}

fn conjugate_have(_: &[LocArg]) -> LocArg {
    constant("has")(&[])
}

/// Форма глагола для единственного числа: второй аргумент.
fn conjugate_basic(args: &[LocArg]) -> LocArg {
    LocArg::Text(text(args, 1))
}

fn gender(_: &[LocArg]) -> LocArg {
    constant("neuter")(&[])
}

fn proper(_: &[LocArg]) -> LocArg {
    constant("false")(&[])
}

/// Регистрирует функции до загрузки языков.
pub fn install(loc: &mut Loc) {
    loc.add_function("INDEFINITE", indefinite);
    loc.add_function("THE", the);
    loc.add_function("SUBJECT", subject);
    loc.add_function("OBJECT", object);
    loc.add_function("POSS-ADJ", poss_adj);
    loc.add_function("POSS-PRONOUN", poss_adj);
    loc.add_function("REFLEXIVE", reflexive);
    loc.add_function("CONJUGATE-BE", conjugate_be);
    loc.add_function("CONJUGATE-HAVE", conjugate_have);
    loc.add_function("CONJUGATE-BASIC", conjugate_basic);
    loc.add_function("GENDER", gender);
    loc.add_function("PROPER", proper);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn articles_follow_the_first_letter() {
        assert_eq!(indefinite(&["apple".into()]), LocArg::Text("an".into()));
        assert_eq!(indefinite(&["small".into()]), LocArg::Text("a".into()));
        assert_eq!(the(&["crowbar".into()]), LocArg::Text("the crowbar".into()));
        assert_eq!(
            conjugate_basic(&["x".into(), "drinks".into(), "drink".into()]),
            LocArg::Text("drinks".into())
        );
    }
}
