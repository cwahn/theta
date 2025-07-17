// benches/channel_bench.rs
use criterion::{Criterion, criterion_group, criterion_main};
use kanal;
use tokio::runtime::Runtime;
use tokio::sync::mpsc;

const N: usize = 1_000_000;
const CAP: usize = 1_024;

fn bench_tokio(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    c.bench_function("tokio::mpsc", |b| {
        b.iter(|| {
            rt.block_on(async {
                let (tx, mut rx) = mpsc::channel::<usize>(CAP);
                let tx2 = tx.clone();
                let producer = tokio::spawn(async move {
                    for i in 0..N {
                        tx2.send(i).await.unwrap();
                    }
                });
                drop(tx);
                while rx.recv().await.is_some() {}
                producer.await.unwrap();
            })
        })
    });
}

fn bench_kanal(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    c.bench_function("kanal::bounded_async", |b| {
        b.iter(|| {
            rt.block_on(async {
                let (tx, mut rx) = kanal::bounded_async::<usize>(CAP);
                let tx2 = tx.clone();
                let producer = tokio::spawn(async move {
                    for i in 0..N {
                        tx2.send(i).await.unwrap();
                    }
                });
                drop(tx);
                while rx.recv().await.is_ok() {}
                producer.await.unwrap();
            })
        })
    });
}

criterion_group!(benches, bench_tokio, bench_kanal);
criterion_main!(benches);
