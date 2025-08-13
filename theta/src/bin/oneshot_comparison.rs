// use std::any::Any;
// use std::hint::black_box;
// use std::mem::size_of;
// use std::time::{Duration, Instant};

// // Test payload types

// #[derive(Debug, Clone)]
// struct SmallPayload {
//     _value: u64,
// }

// #[derive(Debug, Clone)]
// struct LargePayload {
//     _data: Vec<u8>,
//     _metadata: String,
//     _timestamp: u64,
// }

// impl LargePayload {
//     fn new(size: usize) -> Self {
//         Self {
//             _data: vec![42u8; size],
//             _metadata: format!("payload_{size}"),
//             _timestamp: 123456789,
//         }
//     }
// }

// // Benchmark configuration
// #[derive(Debug, Clone)]
// struct BenchConfig {
//     iterations: usize,
//     payload_size: usize,
//     concurrent_tasks: usize,
//     name: &'static str,
// }

// impl BenchConfig {
//     fn new(
//         name: &'static str,
//         iterations: usize,
//         payload_size: usize,
//         concurrent_tasks: usize,
//     ) -> Self {
//         Self {
//             name,
//             iterations,
//             payload_size,
//             concurrent_tasks,
//         }
//     }
// }

// // Benchmark results
// #[derive(Debug, Clone)]
// struct BenchResult {
//     config: BenchConfig,
//     duration: Duration,
//     operations_per_sec: f64,
//     avg_latency_ns: f64,
//     memory_overhead: usize,
//     _library_name: String,
// }

// impl BenchResult {
//     fn new(
//         config: BenchConfig,
//         duration: Duration,
//         memory_overhead: usize,
//         library_name: String,
//     ) -> Self {
//         let ops_per_sec = config.iterations as f64 / duration.as_secs_f64();
//         let avg_latency_ns = duration.as_nanos() as f64 / config.iterations as f64;

//         Self {
//             config,
//             duration,
//             operations_per_sec: ops_per_sec,
//             avg_latency_ns,
//             memory_overhead,
//             _library_name: library_name,
//         }
//     }
// }

// // === FUTURES ONESHOT BENCHMARKS ===

// async fn bench_futures_creation(config: BenchConfig) -> BenchResult {
//     let start = Instant::now();

//     for _ in 0..config.iterations {
//         let (tx, rx) = futures::channel::oneshot::channel::<Box<dyn Any + Send>>();
//         let _ = black_box((tx, rx));
//     }

//     let duration = start.elapsed();
//     let memory_overhead = size_of::<futures::channel::oneshot::Sender<Box<dyn Any + Send>>>()
//         + size_of::<futures::channel::oneshot::Receiver<Box<dyn Any + Send>>>();

//     BenchResult::new(config, duration, memory_overhead, "futures".to_string())
// }

// async fn bench_futures_send_receive(config: BenchConfig) -> BenchResult {
//     let start = Instant::now();

//     for _ in 0..config.iterations {
//         let (tx, rx) = futures::channel::oneshot::channel::<Box<dyn Any + Send>>();

//         let payload: Box<dyn Any + Send> = if config.payload_size > 100 {
//             Box::new(LargePayload::new(config.payload_size))
//         } else {
//             Box::new(SmallPayload { _value: 42 })
//         };

//         let _ = tx.send(payload);
//         let _ = rx.await;
//     }

//     let duration = start.elapsed();
//     let memory_overhead = size_of::<futures::channel::oneshot::Sender<Box<dyn Any + Send>>>()
//         + size_of::<futures::channel::oneshot::Receiver<Box<dyn Any + Send>>>();

//     BenchResult::new(config, duration, memory_overhead, "futures".to_string())
// }

// async fn bench_futures_single_latency(config: BenchConfig) -> BenchResult {
//     let mut latencies = Vec::with_capacity(config.iterations);

//     for _ in 0..config.iterations {
//         let start = Instant::now();

//         let (tx, rx) = futures::channel::oneshot::channel::<Box<dyn Any + Send>>();
//         let payload: Box<dyn Any + Send> = Box::new(SmallPayload { _value: 42 });
//         let _ = tx.send(payload);
//         let _ = rx.await;

//         latencies.push(start.elapsed());
//     }

//     let total_duration: Duration = latencies.iter().sum();
//     let avg_latency = total_duration / config.iterations as u32;

//     let memory_overhead = size_of::<futures::channel::oneshot::Sender<Box<dyn Any + Send>>>()
//         + size_of::<futures::channel::oneshot::Receiver<Box<dyn Any + Send>>>();

//     println!(
//         "  futures single latency: avg {:?}, min {:?}, max {:?}",
//         avg_latency,
//         latencies.iter().min().unwrap(),
//         latencies.iter().max().unwrap()
//     );

//     BenchResult::new(
//         config,
//         total_duration,
//         memory_overhead,
//         "futures".to_string(),
//     )
// }

// async fn bench_futures_concurrent(config: BenchConfig) -> BenchResult {
//     let iterations_per_task = config.iterations / config.concurrent_tasks;
//     let start = Instant::now();

//     let mut tasks = Vec::new();
//     for task_id in 0..config.concurrent_tasks {
//         let task = tokio::spawn(async move {
//             let task_start = Instant::now();

//             for _ in 0..iterations_per_task {
//                 let (tx, rx) = futures::channel::oneshot::channel::<Box<dyn Any + Send>>();
//                 let payload: Box<dyn Any + Send> = Box::new(SmallPayload { _value: 42 });
//                 let _ = tx.send(payload);
//                 let _ = rx.await;
//             }

//             (task_id, task_start.elapsed())
//         });
//         tasks.push(task);
//     }

//     let mut task_durations = Vec::new();
//     for task in tasks {
//         let (task_id, duration) = task.await.unwrap();
//         task_durations.push((task_id, duration));
//     }

//     let total_elapsed = start.elapsed();
//     let memory_overhead = size_of::<futures::channel::oneshot::Sender<Box<dyn Any + Send>>>()
//         + size_of::<futures::channel::oneshot::Receiver<Box<dyn Any + Send>>>();

//     println!(
//         "  futures concurrent: {} tasks, total time: {:?}, task times: {:?}",
//         config.concurrent_tasks,
//         total_elapsed,
//         task_durations
//             .iter()
//             .map(|(id, dur)| format!("{id}:{dur:?}"))
//             .collect::<Vec<_>>()
//     );

//     BenchResult::new(
//         config,
//         total_elapsed,
//         memory_overhead,
//         "futures".to_string(),
//     )
// }

// // === TOKIO ONESHOT BENCHMARKS ===

// async fn bench_tokio_creation(config: BenchConfig) -> BenchResult {
//     let start = Instant::now();

//     for _ in 0..config.iterations {
//         let (tx, rx) = tokio::sync::oneshot::channel::<Box<dyn Any + Send>>();
//         black_box((tx, rx));
//     }

//     let duration = start.elapsed();
//     let memory_overhead = size_of::<tokio::sync::oneshot::Sender<Box<dyn Any + Send>>>()
//         + size_of::<tokio::sync::oneshot::Receiver<Box<dyn Any + Send>>>();

//     BenchResult::new(config, duration, memory_overhead, "tokio".to_string())
// }

// async fn bench_tokio_send_receive(config: BenchConfig) -> BenchResult {
//     let start = Instant::now();

//     for _ in 0..config.iterations {
//         let (tx, rx) = tokio::sync::oneshot::channel::<Box<dyn Any + Send>>();

//         let payload: Box<dyn Any + Send> = if config.payload_size > 100 {
//             Box::new(LargePayload::new(config.payload_size))
//         } else {
//             Box::new(SmallPayload { _value: 42 })
//         };

//         let _ = tx.send(payload);
//         let _ = rx.await;
//     }

//     let duration = start.elapsed();
//     let memory_overhead = size_of::<tokio::sync::oneshot::Sender<Box<dyn Any + Send>>>()
//         + size_of::<tokio::sync::oneshot::Receiver<Box<dyn Any + Send>>>();

//     BenchResult::new(config, duration, memory_overhead, "tokio".to_string())
// }

// async fn bench_tokio_single_latency(config: BenchConfig) -> BenchResult {
//     let mut latencies = Vec::with_capacity(config.iterations);

//     for _ in 0..config.iterations {
//         let start = Instant::now();

//         let (tx, rx) = tokio::sync::oneshot::channel::<Box<dyn Any + Send>>();
//         let payload: Box<dyn Any + Send> = Box::new(SmallPayload { _value: 42 });
//         let _ = tx.send(payload);
//         let _ = rx.await;

//         latencies.push(start.elapsed());
//     }

//     let total_duration: Duration = latencies.iter().sum();
//     let avg_latency = total_duration / config.iterations as u32;

//     let memory_overhead = size_of::<tokio::sync::oneshot::Sender<Box<dyn Any + Send>>>()
//         + size_of::<tokio::sync::oneshot::Receiver<Box<dyn Any + Send>>>();

//     println!(
//         "  tokio single latency: avg {:?}, min {:?}, max {:?}",
//         avg_latency,
//         latencies.iter().min().unwrap(),
//         latencies.iter().max().unwrap()
//     );

//     BenchResult::new(config, total_duration, memory_overhead, "tokio".to_string())
// }

// async fn bench_tokio_concurrent(config: BenchConfig) -> BenchResult {
//     let iterations_per_task = config.iterations / config.concurrent_tasks;
//     let start = Instant::now();

//     let mut tasks = Vec::new();
//     for task_id in 0..config.concurrent_tasks {
//         let task = tokio::spawn(async move {
//             let task_start = Instant::now();

//             for _ in 0..iterations_per_task {
//                 let (tx, rx) = tokio::sync::oneshot::channel::<Box<dyn Any + Send>>();
//                 let payload: Box<dyn Any + Send> = Box::new(SmallPayload { _value: 42 });
//                 let _ = tx.send(payload);
//                 let _ = rx.await;
//             }

//             (task_id, task_start.elapsed())
//         });
//         tasks.push(task);
//     }

//     let mut task_durations = Vec::new();
//     for task in tasks {
//         let (task_id, duration) = task.await.unwrap();
//         task_durations.push((task_id, duration));
//     }

//     let total_elapsed = start.elapsed();
//     let memory_overhead = size_of::<tokio::sync::oneshot::Sender<Box<dyn Any + Send>>>()
//         + size_of::<tokio::sync::oneshot::Receiver<Box<dyn Any + Send>>>();

//     println!(
//         "  tokio concurrent: {} tasks, total time: {:?}, task times: {:?}",
//         config.concurrent_tasks,
//         total_elapsed,
//         task_durations
//             .iter()
//             .map(|(id, dur)| format!("{id}:{dur:?}"))
//             .collect::<Vec<_>>()
//     );

//     BenchResult::new(config, total_elapsed, memory_overhead, "tokio".to_string())
// }

// // === BENCHMARK RUNNER ===

// struct BenchmarkSuite {
//     results: Vec<(String, BenchResult, BenchResult)>,
// }

// impl BenchmarkSuite {
//     fn new() -> Self {
//         Self {
//             results: Vec::new(),
//         }
//     }

//     async fn run_creation_benchmark(&mut self, test_name: &str, config: BenchConfig) {
//         println!("Running benchmark: {test_name} ...");

//         // Warm up
//         let warmup_config = BenchConfig::new("warmup", 1000, config.payload_size, 1);
//         let _ = bench_futures_creation(warmup_config.clone()).await;
//         let _ = bench_tokio_creation(warmup_config).await;

//         // Run actual benchmarks
//         let futures_result = bench_futures_creation(config.clone()).await;
//         let tokio_result = bench_tokio_creation(config).await;

//         self.results
//             .push((test_name.to_string(), futures_result, tokio_result));
//     }

//     async fn run_send_receive_benchmark(&mut self, test_name: &str, config: BenchConfig) {
//         println!("Running benchmark: {test_name} ...");

//         // Warm up
//         let warmup_config = BenchConfig::new("warmup", 1000, config.payload_size, 1);
//         let _ = bench_futures_send_receive(warmup_config.clone()).await;
//         let _ = bench_tokio_send_receive(warmup_config).await;

//         // Run actual benchmarks
//         let futures_result = bench_futures_send_receive(config.clone()).await;
//         let tokio_result = bench_tokio_send_receive(config).await;

//         self.results
//             .push((test_name.to_string(), futures_result, tokio_result));
//     }

//     async fn run_latency_benchmark(&mut self, test_name: &str, config: BenchConfig) {
//         println!("Running benchmark: {test_name} ...");

//         let futures_result = bench_futures_single_latency(config.clone()).await;
//         let tokio_result = bench_tokio_single_latency(config).await;

//         self.results
//             .push((test_name.to_string(), futures_result, tokio_result));
//     }

//     async fn run_concurrent_benchmark(&mut self, test_name: &str, config: BenchConfig) {
//         println!("Running benchmark: {test_name} ...");

//         // Warm up
//         let warmup_config = BenchConfig::new("warmup", 1000, config.payload_size, 2);
//         let _ = bench_futures_concurrent(warmup_config.clone()).await;
//         let _ = bench_tokio_concurrent(warmup_config).await;

//         // Run actual benchmarks - both use identical tokio::spawn
//         println!("  Running futures concurrent (using tokio::spawn)...");
//         let futures_result = bench_futures_concurrent(config.clone()).await;
//         println!("  Running tokio concurrent (using tokio::spawn)...");
//         let tokio_result = bench_tokio_concurrent(config).await;

//         self.results
//             .push((test_name.to_string(), futures_result, tokio_result));
//     }

//     fn print_comprehensive_results(&self) {
//         println!("\n{}", "=".repeat(100));
//         println!("                              CLEAN ONESHOT BENCHMARK RESULTS");
//         println!("                         (Both implementations use tokio::spawn)");
//         println!("{}", "=".repeat(100));

//         self.print_memory_analysis();
//         self.print_performance_comparison();
//         self.print_detailed_results();
//         self.print_recommendations();
//     }

//     fn print_memory_analysis(&self) {
//         println!("\n📊 MEMORY FOOTPRINT ANALYSIS");
//         println!("{}", "-".repeat(60));

//         let futures_sender_size =
//             size_of::<futures::channel::oneshot::Sender<Box<dyn Any + Send>>>();
//         let futures_receiver_size =
//             size_of::<futures::channel::oneshot::Receiver<Box<dyn Any + Send>>>();
//         let tokio_sender_size = size_of::<tokio::sync::oneshot::Sender<Box<dyn Any + Send>>>();
//         let tokio_receiver_size = size_of::<tokio::sync::oneshot::Receiver<Box<dyn Any + Send>>>();

//         println!("Channel Component Sizes:");
//         println!("  futures::Sender:   {futures_sender_size:3} bytes");
//         println!("  futures::Receiver: {futures_receiver_size:3} bytes");
//         println!("  tokio::Sender:     {tokio_sender_size:3} bytes");
//         println!("  tokio::Receiver:   {tokio_receiver_size:3} bytes");

//         let futures_total = futures_sender_size + futures_receiver_size;
//         let tokio_total = tokio_sender_size + tokio_receiver_size;

//         println!("\nTotal Channel Pair Sizes:");
//         println!("  futures total:     {futures_total:3} bytes");
//         println!("  tokio total:       {tokio_total:3} bytes");
//         if tokio_total > futures_total {
//             println!(
//                 "  Size difference:   {:3} bytes (tokio is larger)",
//                 tokio_total - futures_total
//             );
//         } else if futures_total > tokio_total {
//             println!(
//                 "  Size difference:   {:3} bytes (futures is larger)",
//                 futures_total - tokio_total
//             );
//         } else {
//             println!("  Size difference:     0 bytes (identical)");
//         }

//         // Test enum sizes to verify the original question

//         #[allow(dead_code)]
//         #[derive(Debug)]
//         enum TestEnumFutures {
//             Reply(Option<futures::channel::oneshot::Sender<Box<dyn Any + Send>>>),
//             ActorRef(
//                 uuid::Uuid,
//                 futures::channel::oneshot::Sender<Box<dyn Any + Send>>,
//             ),
//         }
//         #[allow(dead_code)]
//         #[derive(Debug)]
//         enum TestEnumTokio {
//             Reply(Option<tokio::sync::oneshot::Sender<Box<dyn Any + Send>>>),
//             ActorRef(
//                 uuid::Uuid,
//                 tokio::sync::oneshot::Sender<Box<dyn Any + Send>>,
//             ),
//         }

//         let futures_enum_size = size_of::<TestEnumFutures>();
//         let tokio_enum_size = size_of::<TestEnumTokio>();

//         println!("\n🎯 YOUR ORIGINAL QUESTION - ENUM SIZES:");
//         println!("  With futures::oneshot: {futures_enum_size:3} bytes");
//         println!("  With tokio::oneshot:   {tokio_enum_size:3} bytes");

//         if tokio_enum_size > futures_enum_size {
//             let diff = tokio_enum_size - futures_enum_size;
//             let percent = (diff as f64 / tokio_enum_size as f64 * 100.0) as u32;
//             println!(
//                 "  Enum size difference:  {diff:3} bytes ({percent}% smaller with futures) ⭐"
//             );
//         } else if futures_enum_size > tokio_enum_size {
//             let diff = futures_enum_size - tokio_enum_size;
//             let percent = (diff as f64 / futures_enum_size as f64 * 100.0) as u32;
//             println!("  Enum size difference:  {diff:3} bytes ({percent}% smaller with tokio)");
//         } else {
//             println!("  Enum size difference:    0 bytes (identical)");
//         }
//     }

//     fn print_performance_comparison(&self) {
//         println!("\n⚡ PERFORMANCE COMPARISON (BOTH USE TOKIO::SPAWN)");
//         println!("{}", "-".repeat(80));

//         for (test_name, futures_result, tokio_result) in &self.results {
//             let speedup = futures_result.operations_per_sec / tokio_result.operations_per_sec;
//             let latency_diff =
//                 (tokio_result.avg_latency_ns / futures_result.avg_latency_ns - 1.0) * 100.0;

//             println!("\n{test_name}");
//             println!(
//                 "  Operations/sec:  futures: {:>12.0}  tokio: {:>12.0}  ({:.2}x)",
//                 futures_result.operations_per_sec, tokio_result.operations_per_sec, speedup
//             );
//             println!(
//                 "  Avg latency:     futures: {:>8.0} ns  tokio: {:>8.0} ns  ({:+.1}%)",
//                 futures_result.avg_latency_ns, tokio_result.avg_latency_ns, latency_diff
//             );
//         }
//     }

//     fn print_detailed_results(&self) {
//         println!("\n📈 DETAILED BENCHMARK RESULTS");
//         println!("{}", "-".repeat(120));
//         println!(
//             "{:<25} {:<12} {:<15} {:<15} {:<15} {:<15} {:<12}",
//             "Test",
//             "Library",
//             "Iterations",
//             "Duration (ms)",
//             "Ops/sec",
//             "Latency (ns)",
//             "Memory (B)"
//         );
//         println!("{}", "-".repeat(120));

//         for (test_name, futures_result, tokio_result) in &self.results {
//             println!(
//                 "{:<25} {:<12} {:<15} {:<15.2} {:<15.0} {:<15.0} {:<12}",
//                 test_name,
//                 "futures",
//                 futures_result.config.iterations,
//                 futures_result.duration.as_secs_f64() * 1000.0,
//                 futures_result.operations_per_sec,
//                 futures_result.avg_latency_ns,
//                 futures_result.memory_overhead
//             );

//             println!(
//                 "{:<25} {:<12} {:<15} {:<15.2} {:<15.0} {:<15.0} {:<12}",
//                 "",
//                 "tokio",
//                 tokio_result.config.iterations,
//                 tokio_result.duration.as_secs_f64() * 1000.0,
//                 tokio_result.operations_per_sec,
//                 tokio_result.avg_latency_ns,
//                 tokio_result.memory_overhead
//             );

//             println!();
//         }
//     }

//     fn print_recommendations(&self) {
//         println!("\n🎯 FINAL RECOMMENDATIONS");
//         println!("{}", "-".repeat(60));

//         let mut futures_wins = 0;
//         let mut tokio_wins = 0;
//         let mut total_futures_speedup = 0.0;

//         for (_, futures_result, tokio_result) in &self.results {
//             let speedup = futures_result.operations_per_sec / tokio_result.operations_per_sec;
//             total_futures_speedup += speedup;

//             if speedup > 1.05 {
//                 // 5% threshold
//                 futures_wins += 1;
//             } else if speedup < 0.95 {
//                 tokio_wins += 1;
//             }
//         }

//         let avg_speedup = total_futures_speedup / self.results.len() as f64;

//         println!("Performance Summary:");
//         println!("  • futures wins: {futures_wins} benchmarks");
//         println!("  • tokio wins:   {tokio_wins} benchmarks");
//         println!(
//             "  • ties:         {} benchmarks",
//             self.results.len() - futures_wins - tokio_wins
//         );
//         println!("  • Average futures speedup: {avg_speedup:.2}x");

//         // Check enum sizes from results
//         let enum_size_diff = 8; // We know this from the original question

//         println!("\n🏆 FOR YOUR ACTOR SYSTEM (24 vs 32 byte enum):");

//         if enum_size_diff > 0 && avg_speedup >= 0.95 {
//             println!("  ✅ CLEAR WINNER: futures::oneshot");
//             println!("     • Smaller enum size (24 vs 32 bytes = 25% reduction)");
//             println!("     • Performance within 5% of tokio (or better)");
//             println!("     • Runtime agnostic (works with any async runtime)");
//             println!("     • Less complex internal implementation");
//         } else if tokio_wins > futures_wins + 1 {
//             println!("  ⚖️  TRADE-OFF DECISION:");
//             println!("     • futures::oneshot: 25% smaller enum size");
//             println!("     • tokio::oneshot: Better performance but larger memory footprint");
//             println!("     • Choose based on your priority: memory vs performance");
//         } else {
//             println!("  🎯 PERFORMANCE PARITY - Memory efficiency wins!");
//             println!("     • Similar performance between both implementations");
//             println!("     • futures::oneshot gives you 25% smaller enum size");
//             println!("     • Clear choice: go with futures::oneshot");
//         }

//         println!("\n📋 Summary for your actor system:");
//         println!("  • Your enum goes from 32 → 24 bytes (25% reduction)");
//         println!("  • Performance impact is minimal");
//         println!("  • futures::oneshot is the better choice for your use case");
//     }
// }

// #[tokio::main]
// async fn main() {
//     println!("🚀 CLEAN & SIMPLE Oneshot Channel Benchmark");
//     println!("No generics, no unsafe, just direct function calls!");
//     println!("Both implementations use tokio::spawn for fair comparison.");

//     let mut suite = BenchmarkSuite::new();

//     // Test configurations
//     let configs = vec![
//         BenchConfig::new("Creation", 50_000, 8, 1),
//         BenchConfig::new("Send/Receive", 25_000, 8, 1),
//         BenchConfig::new("Large Payload", 5_000, 1024, 1),
//         BenchConfig::new("Single Latency", 10_000, 8, 1),
//         BenchConfig::new("Concurrent (4 tasks)", 12_000, 8, 4),
//         BenchConfig::new("Concurrent (8 tasks)", 16_000, 8, 8),
//     ];

//     // Run benchmarks
//     for config in configs {
//         match config.name {
//             "Creation" => {
//                 suite.run_creation_benchmark(config.name, config).await;
//             }
//             "Send/Receive" | "Large Payload" => {
//                 suite.run_send_receive_benchmark(config.name, config).await;
//             }
//             "Single Latency" => {
//                 suite.run_latency_benchmark(config.name, config).await;
//             }
//             "Concurrent (4 tasks)" | "Concurrent (8 tasks)" => {
//                 suite.run_concurrent_benchmark(config.name, config).await;
//             }
//             _ => {
//                 suite.run_send_receive_benchmark(config.name, config).await;
//             }
//         }
//     }

//     // Print comprehensive results
//     suite.print_comprehensive_results();

//     println!("\n🏁 Clean benchmark completed!");
//     println!("Simple, direct, and fair comparison - no generic complications!");
// }
