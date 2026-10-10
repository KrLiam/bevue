//! Benchmarks for syncing `Props` data into templates and for rendering them.
//!
//! Run with `cargo bench --bench reactivity`, or a single case with
//! `cargo bench --bench reactivity -- text_change`.
//!
//! Each case is measured in phases, named `{case}/{phase}/{items}`:
//! - `sync_resource` / `sync_component`: only the system diffing the Rust data into `BevueProps`.
//! - `render` / `update`: only Bevue's rendering of the template, or of its changes.
//! - `frame`: a whole `app.update()`, with every other system of the app. Compare it with
//!   `idle/frame`, a frame in which nothing changed.
//!
//! Only the phase is timed: mutations and restoring the starting state run outside the timer.

use std::time::{Duration, Instant};

use bevue::{
    BevueProps, BevuePropsAppExt, Props, PropsResource, PropsSource, Template, Value,
    core::{path::PropPath, template::BevueTemplate},
    parser::parse_template,
    runtime::{
        BevueRuntimePlugin, update_templates,
        props::{sync_component_props, sync_resource_props},
        render::RenderTemplate,
    },
    value,
};
use bevy::{
    MinimalPlugins,
    app::App,
    asset::{AssetPlugin, Assets, Handle},
    ecs::{component::Component, entity::Entity, hierarchy::Children, resource::Resource, system::SystemId},
};
use criterion::{BenchmarkGroup, BenchmarkId, Criterion, criterion_group, criterion_main, measurement::WallTime};

const SIZES: [usize; 3] = [100, 1_000, 10_000];

/// Three entities and two text bindings per item.
const TEMPLATE: &str = r#"
<div>
    <p>{{inv.title}}</p>
    <div v-for="item in inv.items" :key="item.id" class="row">
        <span>{{item.label}}</span><span>{{item.count}}</span>
    </div>
</div>
"#;

/// The same data as a resource and as a component: in Bevy a type cannot be both.
#[derive(Resource, Props, Clone)]
struct Inventory {
    title: String,
    items: Vec<Item>,
}

#[derive(Component, Props, Clone)]
struct InventoryComponent {
    title: String,
    items: Vec<Item>,
}

impl From<Inventory> for InventoryComponent {
    fn from(Inventory { title, items }: Inventory) -> Self {
        Self { title, items }
    }
}

#[derive(Props, Clone)]
struct Item {
    id: u32,
    label: String,
    count: u32,
}

fn item(id: u32) -> Item {
    Item { id, label: format!("Item {id}"), count: id % 64 }
}

fn inventory(n: usize) -> Inventory {
    Inventory { title: "Inventory".to_owned(), items: (0..n as u32).map(item).collect() }
}

/// A small xorshift generator, so runs are repeatable without a `rand` dependency.
struct Rng(u64);

impl Rng {
    fn below(&mut self, bound: usize) -> usize {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 % bound as u64) as usize
    }
}

/// Where the template's props come from.
#[derive(Clone, Copy)]
enum Source {
    /// An `Inventory` resource synced by `sync_resource_props`.
    Resource,
    /// An `InventoryComponent` synced by `sync_component_props`.
    Component,
    /// `BevueProps` mutated directly, with no `Props` sync.
    Direct,
}

struct Bench {
    app: App,
    template: Handle<BevueTemplate>,
    source: Source,
    root: Entity,
    /// Our own instances of the systems, to run each phase alone.
    sync: SystemId,
    update: SystemId,
    rng: Rng,
    next_id: u32,
    version: u32,
}

impl Bench {
    /// An app with no template entity yet.
    fn new(source: Source) -> Self {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, AssetPlugin::default(), BevueRuntimePlugin))
            .register_props_resource::<Inventory>()
            .register_props::<InventoryComponent>();
        app.finish();
        app.cleanup();

        let template = parse_template(TEMPLATE).expect("the benchmark template parses");
        let template = app.world_mut().resource_mut::<Assets<BevueTemplate>>().add(template);
        let world = app.world_mut();
        let sync = match source {
            Source::Resource | Source::Direct => world.register_system(sync_resource_props::<Inventory>),
            Source::Component => world.register_system(sync_component_props::<InventoryComponent>),
        };
        let update = world.register_system(update_templates);
        Self { app, template, source, root: Entity::PLACEHOLDER, sync, update, rng: Rng(0x9E3779B97F4A7C15), next_id: 0, version: 0 }
    }

    /// An app with a rendered list of `n` items.
    fn rendered(source: Source, n: usize) -> Self {
        let mut bench = Self::new(source);
        bench.next_id = n as u32;
        let template = Template::new(bench.template.clone());
        let world = bench.app.world_mut();
        bench.root = match source {
            Source::Resource => {
                world.insert_resource(inventory(n));
                world.spawn((template, PropsResource::<Inventory>::at("inv"))).id()
            }
            Source::Component => {
                let data = InventoryComponent::from(inventory(n));
                world.spawn((template, data, PropsSource::<InventoryComponent>::at("inv"))).id()
            }
            Source::Direct => world.spawn((template, BevueProps::new(props_value(n)))).id(),
        };
        bench.app.update();
        assert_eq!(bench.app.world().get::<Children>(bench.root).map(|c| c.len()), Some(n + 1));

        // Let our own system instances catch up with what the first frame already handled.
        bench.sync();
        bench.update();
        bench
    }

    /// The items of the Rust data, marking it as changed.
    fn data(&mut self) -> &mut Vec<Item> {
        let world = self.app.world_mut();
        match self.source {
            Source::Resource => &mut world.resource_mut::<Inventory>().into_inner().items,
            Source::Component => &mut world.get_mut::<InventoryComponent>(self.root).unwrap().into_inner().items,
            Source::Direct => panic!("direct props have no Rust data"),
        }
    }

    fn items(&mut self) -> &mut Value {
        let path = PropPath::parse("inv.items").unwrap();
        let props = self.app.world_mut().get_mut::<BevueProps>(self.root).unwrap().into_inner();
        props.value.at_mut(&path).unwrap()
    }

    fn sync(&mut self) {
        self.app.world_mut().run_system(self.sync).unwrap();
    }

    fn update(&mut self) {
        self.app.world_mut().run_system(self.update).unwrap();
    }

    fn frame(&mut self) {
        self.app.update();
    }

    /// Inserts a new item at a random index, returning the index.
    fn insert_random(&mut self) -> usize {
        let id = self.next_id;
        self.next_id += 1;
        match self.source {
            Source::Direct => {
                let index = {
                    let len = self.items().as_list().unwrap().len();
                    self.rng.below(len + 1)
                };
                self.items().insert_at(index, item(id).to_value());
                index
            }
            _ => {
                let len = self.data().len();
                let index = self.rng.below(len + 1);
                self.data().insert(index, item(id));
                index
            }
        }
    }

    fn remove(&mut self, index: usize) {
        match self.source {
            Source::Direct => {
                self.items().remove_at(index);
            }
            _ => {
                self.data().remove(index);
            }
        }
    }

    /// Changes the label of one random item, or of every item.
    fn change_labels(&mut self, all: bool) {
        self.version += 1;
        let version = self.version;
        let len = self.data().len();
        let range = if all {
            0..len
        } else {
            let index = self.rng.below(len);
            index..index + 1
        };
        for item in &mut self.data()[range] {
            item.label = format!("Item {} v{version}", item.id);
        }
    }
}

fn props_value(n: usize) -> Value {
    value!({ "inv": (inventory(n).to_value()) })
}

fn timed(f: impl FnOnce()) -> Duration {
    let start = Instant::now();
    f();
    start.elapsed()
}

/// Runs `step` once per iteration, summing the durations it returns.
fn bench(group: &mut BenchmarkGroup<WallTime>, phase: &str, n: usize, mut step: impl FnMut() -> Duration) {
    group.sample_size(if n >= 10_000 { 10 } else { 30 });
    group.bench_function(BenchmarkId::new(phase, n), |b| b.iter_custom(|iters| (0..iters).map(|_| step()).sum()));
}

fn group<'a>(c: &'a mut Criterion, name: &str) -> BenchmarkGroup<'a, WallTime> {
    let mut group = c.benchmark_group(name);
    group.warm_up_time(Duration::from_millis(500)).measurement_time(Duration::from_secs(2));
    group
}

/// Rendering a list of N items that was not rendered before.
fn render_scratch(c: &mut Criterion) {
    let mut group = group(c, "render_scratch");
    for n in SIZES {
        // The first sync into empty props builds the whole value.
        for (phase, source) in [("sync_resource", Source::Resource), ("sync_component", Source::Component)] {
            let mut b = Bench::new(source);
            b.app.world_mut().insert_resource(inventory(n));
            let data = InventoryComponent::from(inventory(n));
            bench(&mut group, phase, n, || {
                let world = b.app.world_mut();
                let entity = match source {
                    Source::Component => world.spawn((data.clone(), PropsSource::<InventoryComponent>::at("inv"))).id(),
                    _ => world.spawn(PropsResource::<Inventory>::at("inv")).id(),
                };
                let elapsed = timed(|| b.sync());
                b.app.world_mut().despawn(entity);
                elapsed
            });
        }

        let value = props_value(n);
        {
            let mut b = Bench::new(Source::Direct);
            bench(&mut group, "render", n, || {
                let world = b.app.world_mut();
                let entity = world.spawn(BevueProps::new(value.clone())).id();
                let template = b.template.clone();
                let elapsed = timed(|| {
                    world.trigger(RenderTemplate { template, ui_root: Some(entity) });
                    world.flush();
                });
                assert_eq!(world.get::<Children>(entity).map(|c| c.len()), Some(n + 1));
                world.despawn(entity);
                elapsed
            });
        }
        {
            let mut b = Bench::new(Source::Resource);
            b.app.world_mut().insert_resource(inventory(n));
            b.frame();
            bench(&mut group, "frame", n, || {
                let template = Template::new(b.template.clone());
                let entity = b.app.world_mut().spawn((template, PropsResource::<Inventory>::at("inv"))).id();
                let elapsed = timed(|| b.frame());
                assert_eq!(b.app.world().get::<Children>(entity).map(|c| c.len()), Some(n + 1));
                b.app.world_mut().despawn(entity);
                elapsed
            });
        }
    }
    group.finish();
}

/// Inserting one item at a random index of a rendered list of N items.
fn list_insert(c: &mut Criterion) {
    let mut group = group(c, "list_insert");
    for n in SIZES {
        for (phase, source) in [("sync_resource", Source::Resource), ("sync_component", Source::Component)] {
            let mut b = Bench::rendered(source, n);
            bench(&mut group, phase, n, || {
                let index = b.insert_random();
                let elapsed = timed(|| b.sync());
                b.update();
                b.remove(index);
                b.sync();
                b.update();
                elapsed
            });
        }
        // `update` follows a `Props` sync, `update_direct` a direct `BevueProps` insertion.
        for (phase, source) in [("update", Source::Resource), ("update_direct", Source::Direct)] {
            let mut b = Bench::rendered(source, n);
            bench(&mut group, phase, n, || {
                let index = b.insert_random();
                b.sync();
                let elapsed = timed(|| b.update());
                b.remove(index);
                b.sync();
                b.update();
                elapsed
            });
        }
        for (phase, source) in [("frame", Source::Resource), ("frame_direct", Source::Direct)] {
            let mut b = Bench::rendered(source, n);
            bench(&mut group, phase, n, || {
                let index = b.insert_random();
                let elapsed = timed(|| b.frame());
                b.remove(index);
                b.frame();
                elapsed
            });
        }
        // Check the list is back to N items and still rendered correctly.
        let mut b = Bench::rendered(Source::Resource, n);
        let index = b.insert_random();
        b.frame();
        assert_eq!(b.app.world().get::<Children>(b.root).map(|c| c.len()), Some(n + 2));
        b.remove(index);
        b.frame();
        assert_eq!(b.app.world().get::<Children>(b.root).map(|c| c.len()), Some(n + 1));
    }
    group.finish();
}

/// Changing the label of rendered items, with no structural change.
fn text_change(c: &mut Criterion) {
    for (name, all) in [("text_change_one", false), ("text_change_all", true)] {
        let mut group = group(c, name);
        for n in SIZES {
            for (phase, source) in [("sync_resource", Source::Resource), ("sync_component", Source::Component)] {
                let mut b = Bench::rendered(source, n);
                bench(&mut group, phase, n, || {
                    b.change_labels(all);
                    let elapsed = timed(|| b.sync());
                    b.update();
                    elapsed
                });
            }
            {
                let mut b = Bench::rendered(Source::Resource, n);
                bench(&mut group, "update", n, || {
                    b.change_labels(all);
                    b.sync();
                    timed(|| b.update())
                });
            }
            {
                let mut b = Bench::rendered(Source::Resource, n);
                bench(&mut group, "frame", n, || {
                    b.change_labels(all);
                    timed(|| b.frame())
                });
            }
        }
        group.finish();
    }
}

/// A frame in which nothing changed, with N rendered items: the baseline for the `frame` phases.
fn idle(c: &mut Criterion) {
    let mut group = group(c, "idle");
    for n in SIZES {
        let mut b = Bench::rendered(Source::Resource, n);
        b.frame();
        bench(&mut group, "frame", n, || timed(|| b.frame()));
    }
    group.finish();
}

criterion_group!(benches, idle, render_scratch, list_insert, text_change);
criterion_main!(benches);
