use shaku::{Component, module};
use std::sync::Arc;
trait Clock: shaku::Interface {
    fn now(&self) -> u64;
}
trait Reader: shaku::Interface {
    fn clock(&self) -> Arc<dyn Clock>;
}
#[derive(Component)]
#[shaku(interface=Reader)]
struct NeedsClock {
    #[shaku(inject)]
    clock: Arc<dyn Clock>,
}
impl Reader for NeedsClock {
    fn clock(&self) -> Arc<dyn Clock> {
        self.clock.clone()
    }
}
module! {Missing {components=[NeedsClock],providers=[]}}
fn main() {
    let _ = Missing::builder().build();
}
