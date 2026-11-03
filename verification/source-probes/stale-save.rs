use canopyd::{config::PortSpec, ports::{BindProbe, Registry}};
use std::collections::BTreeMap;
use camino::Utf8Path;
struct Free;
impl BindProbe for Free { fn is_free(&self, _: u16) -> bool { true } }
fn main() {
    let arg = std::env::args().nth(1).expect("isolated registry path");
    let path = Utf8Path::new(&arg);
    let ports = BTreeMap::from([("web".to_owned(), PortSpec { range: Some((42000, 42010)), ..Default::default() })]);
    let mut first = Registry::load_with(path, Box::new(Free)).unwrap().with_owner("repo-a");
    first.allocate("repo-a", "first", &ports).unwrap();
    let mut second = Registry::load_with(path, Box::new(Free)).unwrap().with_owner("repo-b");
    second.allocate("repo-b", "second", &ports).unwrap();
    let before = Registry::load_with(path, Box::new(Free)).unwrap();
    assert_eq!(before.rows().len(), 2);
    first.save().unwrap();
    let after = Registry::load_with(path, Box::new(Free)).unwrap();
    assert_eq!(after.rows().len(), 1);
    assert!(after.rows().iter().all(|row| row.owner != "repo-b"));
    println!("reproduced: second owner's committed allocation was overwritten by first owner's later save (2 rows -> 1)");
}
