use shaku::{Component, module};
use std::sync::Arc;
trait CycleA: shaku::Interface {
    fn value(&self) -> usize;
}
trait CycleB: shaku::Interface {
    fn value(&self) -> usize;
}
#[derive(Component)]
#[shaku(interface = CycleA)]
struct A {
    #[shaku(inject)]
    b: Arc<dyn CycleB>,
}
#[derive(Component)]
#[shaku(interface = CycleB)]
struct B {
    #[shaku(inject)]
    a: Arc<dyn CycleA>,
}
impl CycleA for A {
    fn value(&self) -> usize {
        self.b.value()
    }
}
impl CycleB for B {
    fn value(&self) -> usize {
        self.a.value()
    }
}
module! { Cyclic { components = [A, B], providers = [] } }
fn main() {
    let _ = Cyclic::builder().build();
}
