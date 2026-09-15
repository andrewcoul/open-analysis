//! Review-only probes, also run unchanged at the PR base.
#![cfg(feature = "parallel")]
use oa_core::{units::*, *};
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

struct Counting;
static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);
fn allocated(n: usize) {
    let live = LIVE.fetch_add(n, Ordering::SeqCst) + n;
    PEAK.fetch_max(live, Ordering::SeqCst);
}
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let p = unsafe { System.alloc(layout) };
        if !p.is_null() {
            allocated(layout.size());
        }
        p
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let p = unsafe { System.alloc_zeroed(layout) };
        if !p.is_null() {
            allocated(layout.size());
        }
        p
    }
    unsafe fn dealloc(&self, p: *mut u8, layout: Layout) {
        LIVE.fetch_sub(layout.size(), Ordering::SeqCst);
        unsafe { System.dealloc(p, layout) };
    }
    unsafe fn realloc(&self, p: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        let new = unsafe { System.realloc(p, layout, size) };
        if !new.is_null() {
            LIVE.fetch_sub(layout.size(), Ordering::SeqCst);
            allocated(size);
        }
        new
    }
}
#[global_allocator]
static ALLOCATOR: Counting = Counting;

fn cantilever() -> Model {
    let AnalysisRequest::Static { model, .. } =
        serde_json::from_str(include_str!("../../../examples/cantilever.json")).unwrap()
    else {
        unreachable!()
    };
    model
}

#[test]
fn selected_case_memory_stays_bounded() {
    let options = StaticOptions {
        threads: 1,
        max_in_flight: 1,
        outputs: OutputSelection {
            displacements: false,
            reactions: false,
            frames: false,
            shells: false,
        },
        ..Default::default()
    };
    analyze_static(&cantilever(), &options).unwrap(); // Warm the worker pool.
    let mut model = cantilever();
    model.nodes.clear();
    model.load_cases.clear();
    model.combinations.clear();
    for i in 0..5000 {
        model.add_node(Node::fixed([
            Length::from_si(i as f64),
            Length::ZERO,
            Length::ZERO,
        ]));
    }
    for i in 0..500 {
        model.load_cases.push(LoadCase {
            name: format!("case{i}"),
            nodal: vec![NodalLoad {
                node: NodeId(i),
                force: [Force::from_si(1.0), Force::ZERO, Force::ZERO],
                moment: [Moment::ZERO; 3],
            }],
            ..Default::default()
        });
    }
    model.combinations.push(LoadCombination {
        name: "selected".into(),
        terms: vec![(LoadCaseId(0), 1.0)],
    });
    let before = LIVE.load(Ordering::SeqCst);
    PEAK.store(before, Ordering::SeqCst);
    analyze_static(&model, &options).unwrap();
    let extra = PEAK.load(Ordering::SeqCst) - before;
    println!("additional peak heap bytes for one selected case: {extra}");
    assert!(
        extra < 32 * 1024 * 1024,
        "unused sparse cases require {extra} bytes"
    );
}

#[test]
fn nested_pool_and_non_send_consumer() {
    let model = cantilever();
    for outer_threads in [1, 2] {
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(outer_threads)
            .build()
            .unwrap();
        pool.install(|| {
            struct Recorder(std::rc::Rc<std::cell::Cell<usize>>, std::thread::ThreadId);
            impl ResultConsumer for Recorder {
                fn consume(&mut self, _: CombinationResult) -> Result<()> {
                    assert_eq!(std::thread::current().id(), self.1);
                    self.0.set(self.0.get() + 1);
                    Ok(())
                }
            }
            for threads in [0, 1, 2] {
                let mut recorder = Recorder(Default::default(), std::thread::current().id());
                analyze_static_into(
                    &model,
                    &StaticOptions {
                        threads,
                        ..Default::default()
                    },
                    &mut recorder,
                )
                .unwrap();
                assert!(recorder.0.get() > 0);
            }
        });
    }
}

#[cfg(windows)]
#[test]
fn pool_cache_releases_surplus_threads() {
    fn thread_count() -> usize {
        let output = std::process::Command::new("powershell.exe")
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                &format!("(Get-Process -Id {}).Threads.Count", std::process::id()),
            ])
            .output()
            .unwrap();
        assert!(output.status.success());
        String::from_utf8(output.stdout)
            .unwrap()
            .trim()
            .parse()
            .unwrap()
    }
    rayon::broadcast(|_| {}); // Exclude the shared global pool from the delta.
    let before = thread_count();
    let model = cantilever();
    for threads in 1..=8 {
        analyze_static(
            &model,
            &StaticOptions {
                threads,
                ..Default::default()
            },
        )
        .unwrap();
    }
    std::thread::sleep(std::time::Duration::from_millis(200));
    let after = thread_count();
    println!("OS threads before requests: {before}; after requests: {after}");
    assert!(
        after <= before + 8,
        "surplus cached pools retain {} threads",
        after - before
    );
}

#[test]
fn panic_child() {
    if std::env::var_os("OA_REVIEW_PANIC_CHILD").is_none() {
        return;
    }
    let mut model = cantilever();
    model.combinations = (0..2)
        .map(|i| LoadCombination {
            name: format!("c{i}"),
            terms: vec![(LoadCaseId(0), 1.0)],
        })
        .collect();
    struct Trigger;
    impl ResultConsumer for Trigger {
        fn consume(&mut self, _: CombinationResult) -> Result<()> {
            // Fault injection through faer's public API: the next backsolve panics.
            faer::disable_global_parallelism();
            Ok(())
        }
    }
    let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        analyze_static_into(
            &model,
            &StaticOptions {
                threads: 1,
                max_in_flight: 1,
                ..Default::default()
            },
            &mut Trigger,
        )
    }));
    faer::set_global_parallelism(faer::Par::Seq);
    assert!(
        caught.is_err(),
        "the injected worker panic should reach the caller"
    );
}

#[test]
fn worker_panic_reaches_caller() {
    use std::{
        process::Command,
        time::{Duration, Instant},
    };
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "panic_child", "--nocapture", "--test-threads=1"])
        .env("OA_REVIEW_PANIC_CHILD", "1")
        .spawn()
        .unwrap();
    let started = Instant::now();
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            assert!(status.success());
            return;
        }
        if started.elapsed() > Duration::from_secs(5) {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("worker panic left the caller blocked for 5 seconds");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}
