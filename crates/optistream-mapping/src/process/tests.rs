use super::*;

#[test]
fn kinds_from_names() {
    assert_eq!(Kind::of("a.ytd"), Some(Kind::Ytd));
    assert_eq!(Kind::of("b.ydr"), Some(Kind::Ydr));
    assert_eq!(Kind::of("c.ydd"), Some(Kind::Ydd));
    assert_eq!(Kind::of("d.yft"), None);
}
