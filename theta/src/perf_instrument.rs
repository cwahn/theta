/// Timing instrumentation for profiling hot paths.
/// Gated behind `perf-instrument` feature flag.
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};

macro_rules! define_counter {
    ($name:ident) => {
        pub static $name: AtomicU64 = AtomicU64::new(0);
    };
}

// send_control_frame
define_counter!(CTRL_SEND_COUNT);
define_counter!(CTRL_SERIALIZE_NS);
define_counter!(CTRL_MUTEX_WAIT_NS);
define_counter!(CTRL_WRITE_NS);
define_counter!(CTRL_TOTAL_NS);

// SendStream::send_frame (individual write_all calls)
define_counter!(WRITE_FRAME_COUNT);
define_counter!(WRITE_LEN_PREFIX_NS);
define_counter!(WRITE_DATA_NS);

/// Print a summary of all accumulated perf counters.
pub fn dump_perf_stats() {
    let ctrl_count = CTRL_SEND_COUNT.load(Relaxed);
    let ctrl_ser_ns = CTRL_SERIALIZE_NS.load(Relaxed);
    let ctrl_mutex_ns = CTRL_MUTEX_WAIT_NS.load(Relaxed);
    let ctrl_write_ns = CTRL_WRITE_NS.load(Relaxed);
    let ctrl_total_ns = CTRL_TOTAL_NS.load(Relaxed);

    let wf_count = WRITE_FRAME_COUNT.load(Relaxed);
    let wf_len_ns = WRITE_LEN_PREFIX_NS.load(Relaxed);
    let wf_data_ns = WRITE_DATA_NS.load(Relaxed);

    let sep = "=".repeat(70);
    println!("\n{sep}");
    println!("  PERF INSTRUMENTATION REPORT");
    println!("{sep}");

    println!("\n[send_control_frame] calls: {ctrl_count}");
    if ctrl_count > 0 {
        println!(
            "  serialize:   total {:>10} µs, avg {:>8.1} µs",
            ctrl_ser_ns / 1000,
            ctrl_ser_ns as f64 / ctrl_count as f64 / 1000.0
        );
        println!(
            "  mutex_wait:  total {:>10} µs, avg {:>8.1} µs",
            ctrl_mutex_ns / 1000,
            ctrl_mutex_ns as f64 / ctrl_count as f64 / 1000.0
        );
        println!(
            "  write:       total {:>10} µs, avg {:>8.1} µs",
            ctrl_write_ns / 1000,
            ctrl_write_ns as f64 / ctrl_count as f64 / 1000.0
        );
        println!(
            "  total:       total {:>10} µs, avg {:>8.1} µs",
            ctrl_total_ns / 1000,
            ctrl_total_ns as f64 / ctrl_count as f64 / 1000.0
        );
        let overhead = ctrl_total_ns.saturating_sub(ctrl_ser_ns + ctrl_mutex_ns + ctrl_write_ns);
        println!(
            "  overhead:    total {:>10} µs (scheduling/other)",
            overhead / 1000
        );

        if ctrl_total_ns > 0 {
            println!("  Breakdown:");
            println!(
                "    serialize:  {:>5.1}%",
                ctrl_ser_ns as f64 / ctrl_total_ns as f64 * 100.0
            );
            println!(
                "    mutex_wait: {:>5.1}%",
                ctrl_mutex_ns as f64 / ctrl_total_ns as f64 * 100.0
            );
            println!(
                "    write:      {:>5.1}%",
                ctrl_write_ns as f64 / ctrl_total_ns as f64 * 100.0
            );
        }
    }

    println!("\n[SendStream::send_frame] calls: {wf_count}");
    if wf_count > 0 {
        println!(
            "  len_prefix:  total {:>10} µs, avg {:>8.1} µs",
            wf_len_ns / 1000,
            wf_len_ns as f64 / wf_count as f64 / 1000.0
        );
        println!(
            "  data_write:  total {:>10} µs, avg {:>8.1} µs",
            wf_data_ns / 1000,
            wf_data_ns as f64 / wf_count as f64 / 1000.0
        );
    }
}

/// Reset all counters to zero.
pub fn reset_perf_stats() {
    CTRL_SEND_COUNT.store(0, Relaxed);
    CTRL_SERIALIZE_NS.store(0, Relaxed);
    CTRL_MUTEX_WAIT_NS.store(0, Relaxed);
    CTRL_WRITE_NS.store(0, Relaxed);
    CTRL_TOTAL_NS.store(0, Relaxed);
    WRITE_FRAME_COUNT.store(0, Relaxed);
    WRITE_LEN_PREFIX_NS.store(0, Relaxed);
    WRITE_DATA_NS.store(0, Relaxed);
}
