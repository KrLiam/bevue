//! Benchmarks for `<query>` props against the same UI written by hand in plain Bevy.
//!
//! Run with `cargo bench --bench queries`, or one group with
//! `cargo bench --bench queries -- query_change_one_sorted`.
//!
//! N unit entities (`Name`, `Health`, `Unit`) are listed one row per unit, as in
//! `examples/units.rs`. Each iteration changes the `Health` of one random unit, and the phase
//! measures how long it takes until its row shows it. Cases are named `{group}/{phase}/{units}`:
//! - `bevue_sync`: only `sync_queries`, diffing changed components into the query rows.
//! - `bevue_update`: only `update_templates`, applying the row change to the UI.
//! - `bevue_frame`: a whole `app.update()` of the Bevue app.
//! - `bevy_system`: only the hand-written systems of the Bevy baseline.
//! - `bevy_frame`: a whole `app.update()` of the Bevy baseline app.
//!
//! The `_sorted` group sorts rows by `health.current desc, name`, so a change usually moves a row.
//! The baseline then sorts all units again and reorders the row entities, the straightforward way
//! to write it by hand.
//!
//! The two apps run different plugins (Bevue adds asset and styling systems), so compare each
//! `frame` phase with its app's `query_idle` frame, and `bevue_sync + bevue_update` with
//! `bevy_system` for the work itself.

use std::time::{Duration, Instant};

use bevue::{
    BevuePropsAppExt, Props, Template,
    core::template::BevueTemplate,
    parser::parse_template,
    runtime::{BevueRuntimePlugin, query::sync_queries, update_templates},
};
use bevy::{
    MinimalPlugins,
    app::{App, Update},
    asset::{AssetPlugin, Assets},
    ecs::{
        component::Component,
        entity::Entity,
        hierarchy::{ChildOf, Children},
        name::Name,
        query::{Changed, With},
        schedule::IntoScheduleConfigs,
        system::{Commands, Query, Single, SystemId},
        world::World,
    },
    ui::{Node, widget::Text},
};
use criterion::{BenchmarkGroup, BenchmarkId, Criterion, criterion_group, criterion_main, measurement::WallTime};

const SIZES: [usize; 3] = [100, 1_000, 10_000];
const MAX_HEALTH: u32 = 100;

#[derive(Component, Props)]
struct Health {
    current: u32,
    max: u32,
}

#[derive(Component, Props)]
struct Unit;

/// A small xorshift generator, so runs are repeatable without a `rand` dependency.
#[derive(Clone)]
struct Rng(u64);

impl Rng {
    fn new() -> Self {
        Self(0x9E3779B97F4A7C15)
    }

    fn below(&mut self, bound: usize) -> usize {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 % bound as u64) as usize
    }
}

/// Spawns `n` units with random health, the same for every call.
fn spawn_units(world: &mut World, n: usize) -> Vec<Entity> {
    let mut rng = Rng::new();
    (0..n)
        .map(|i| {
            let health = Health { current: rng.below(MAX_HEALTH as usize + 1) as u32, max: MAX_HEALTH };
            world.spawn((Name::new(format!("Unit {i}")), health, Unit)).id()
        })
        .collect()
}

/// Sets the health of a random unit to a different random value.
fn change_one(world: &mut World, units: &[Entity], rng: &mut Rng) {
    let unit = units[rng.below(units.len())];
    let mut health = world.get_mut::<Health>(unit).unwrap();
    health.current = (health.current + 1 + rng.below(MAX_HEALTH as usize) as u32) % (MAX_HEALTH + 1);
}

/// The texts of the children of `list`, in order.
fn row_texts(world: &World, list: Entity) -> Vec<String> {
    let children = world.get::<Children>(list).map(|c| c.to_vec()).unwrap_or_default();
    children.iter().map(|row| world.get::<Text>(*row).map(|t| t.0.clone()).unwrap_or_default()).collect()
}

// Bevue

fn template(sorted: bool) -> String {
    let order_by = if sorted { r#" order-by="health.current desc, name""# } else { "" };
    format!(
        r#"<props><query name="units" data="Name, Health" with="Unit"{order_by}/></props>
        <div><p v-for="u in units" :key="u.entity">{{{{ u.name }}}}: {{{{ u.health.current }}}}/{{{{ u.health.max }}}}</p></div>"#
    )
}

struct BevueApp {
    app: App,
    root: Entity,
    units: Vec<Entity>,
    /// Our own instances of the systems, to run each phase alone.
    sync: SystemId,
    update: SystemId,
    rng: Rng,
}

impl BevueApp {
    fn new(n: usize, sorted: bool) -> Self {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, AssetPlugin::default(), BevueRuntimePlugin))
            .register_props::<Name>()
            .register_props::<Health>()
            .register_props::<Unit>();
        app.finish();
        app.cleanup();

        let units = spawn_units(app.world_mut(), n);
        let template = parse_template(&template(sorted)).expect("the benchmark template parses");
        let handle = app.world_mut().resource_mut::<Assets<BevueTemplate>>().add(template);
        let root = app.world_mut().spawn(Template::new(handle)).id();
        app.update();
        assert_eq!(app.world().get::<Children>(root).map(|c| c.len()), Some(n));

        let world = app.world_mut();
        let sync = world.register_system(sync_queries);
        let update = world.register_system(update_templates);
        let mut bench = Self { app, root, units, sync, update, rng: Rng::new() };
        // Let our own system instances catch up with what the first frame already handled.
        bench.sync();
        bench.update();
        bench
    }

    fn change_one(&mut self) {
        change_one(self.app.world_mut(), &self.units, &mut self.rng);
    }

    fn sync(&mut self) {
        self.app.world_mut().run_system(self.sync).unwrap();
    }

    fn update(&mut self) {
        self.app.world_mut().run_system(self.update).unwrap();
    }

    fn texts(&self) -> Vec<String> {
        row_texts(self.app.world(), self.root)
    }
}

// Plain Bevy

/// The list the rows are children of.
#[derive(Component)]
struct List;

/// The row entity showing a unit.
#[derive(Component)]
struct Row(Entity);

fn row_text(name: &Name, health: &Health) -> String {
    format!("{name}: {}/{}", health.current, health.max)
}

/// Rewrites the text of the rows whose unit's health changed.
#[allow(clippy::type_complexity)]
fn update_rows(units: Query<(&Name, &Health, &Row), (With<Unit>, Changed<Health>)>, mut texts: Query<&mut Text>) {
    for (name, health, row) in &units {
        let Ok(mut text) = texts.get_mut(row.0) else { continue };
        let new = row_text(name, health);
        if text.0 != new {
            text.0 = new;
        }
    }
}

/// When some health changed, sorts the rows by health (descending), then name.
fn sort_rows(
    changed: Query<(), (With<Unit>, Changed<Health>)>,
    units: Query<(&Name, &Health, &Row), With<Unit>>,
    list: Single<(Entity, &Children), With<List>>,
    mut commands: Commands,
) {
    if changed.is_empty() {
        return;
    }
    let mut rows: Vec<(u32, &str, Entity)> = units.iter().map(|(n, h, r)| (h.current, n.as_str(), r.0)).collect();
    rows.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(b.1)));
    let (list, children) = *list;
    if !rows.iter().map(|r| r.2).eq(children.iter().copied()) {
        let order: Vec<Entity> = rows.iter().map(|r| r.2).collect();
        commands.entity(list).replace_children(&order);
    }
}

struct BevyApp {
    app: App,
    list: Entity,
    units: Vec<Entity>,
    /// Our own instances of the systems, to run them alone.
    systems: Vec<SystemId>,
    rng: Rng,
}

impl BevyApp {
    fn new(n: usize, sorted: bool) -> Self {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        if sorted {
            app.add_systems(Update, (update_rows, sort_rows).chain());
        } else {
            app.add_systems(Update, update_rows);
        }
        app.finish();
        app.cleanup();

        let world = app.world_mut();
        let units = spawn_units(world, n);
        let list = world.spawn((Node::default(), List)).id();
        for &unit in &units {
            let text = row_text(world.get::<Name>(unit).unwrap(), world.get::<Health>(unit).unwrap());
            let row = world.spawn((Node::default(), Text::new(text), ChildOf(list))).id();
            world.entity_mut(unit).insert(Row(row));
        }
        app.update();

        let world = app.world_mut();
        let mut systems = vec![world.register_system(update_rows)];
        if sorted {
            systems.push(world.register_system(sort_rows));
        }
        let mut bench = Self { app, list, units, systems, rng: Rng::new() };
        bench.run_systems();
        bench
    }

    fn change_one(&mut self) {
        change_one(self.app.world_mut(), &self.units, &mut self.rng);
    }

    fn run_systems(&mut self) {
        for i in 0..self.systems.len() {
            self.app.world_mut().run_system(self.systems[i]).unwrap();
        }
    }

    fn texts(&self) -> Vec<String> {
        row_texts(self.app.world(), self.list)
    }
}

/// Checks that both apps show the same rows, before and after the same changes.
fn check_same(n: usize, sorted: bool) {
    let mut bevue = BevueApp::new(n, sorted);
    let mut bevy = BevyApp::new(n, sorted);
    assert_eq!(bevue.texts(), bevy.texts(), "initial rows differ");
    for _ in 0..5 {
        bevue.change_one();
        bevy.change_one();
        bevue.app.update();
        bevy.app.update();
        assert_eq!(bevue.texts(), bevy.texts(), "rows differ after a change");
    }
}

// Harness

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

/// Changing the health of one unit among N.
fn query_change(c: &mut Criterion) {
    for (name, sorted) in [("query_change_one", false), ("query_change_one_sorted", true)] {
        let mut group = group(c, name);
        for n in SIZES {
            check_same(n, sorted);
            {
                let mut b = BevueApp::new(n, sorted);
                bench(&mut group, "bevue_sync", n, || {
                    b.change_one();
                    let elapsed = timed(|| b.sync());
                    b.update();
                    elapsed
                });
            }
            {
                let mut b = BevueApp::new(n, sorted);
                bench(&mut group, "bevue_update", n, || {
                    b.change_one();
                    b.sync();
                    timed(|| b.update())
                });
            }
            {
                let mut b = BevueApp::new(n, sorted);
                bench(&mut group, "bevue_frame", n, || {
                    b.change_one();
                    timed(|| b.app.update())
                });
            }
            {
                let mut b = BevyApp::new(n, sorted);
                bench(&mut group, "bevy_system", n, || {
                    b.change_one();
                    timed(|| b.run_systems())
                });
            }
            {
                let mut b = BevyApp::new(n, sorted);
                bench(&mut group, "bevy_frame", n, || {
                    b.change_one();
                    timed(|| b.app.update())
                });
            }
        }
        group.finish();
    }
}

/// A frame in which nothing changed, for both apps: the baseline of the `frame` phases.
fn query_idle(c: &mut Criterion) {
    let mut group = group(c, "query_idle");
    for n in SIZES {
        let mut b = BevueApp::new(n, false);
        b.app.update();
        bench(&mut group, "bevue_frame", n, || timed(|| b.app.update()));
        let mut b = BevyApp::new(n, false);
        b.app.update();
        bench(&mut group, "bevy_frame", n, || timed(|| b.app.update()));
    }
    group.finish();
}

criterion_group!(benches, query_idle, query_change);
criterion_main!(benches);
