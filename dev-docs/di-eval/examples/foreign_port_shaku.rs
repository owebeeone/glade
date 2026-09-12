use di_eval_ports::Clock;
use shaku::{Component, module};
#[derive(Component)]
#[shaku(interface = Clock)]
struct Real;
impl Clock for Real {
    fn now(&self) -> u64 {
        0
    }
}
module! { App { components = [Real], providers = [] } }
fn main() {
    let _ = App::builder().build();
}
