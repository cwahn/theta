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

// Export task (server-side: recv msg → dispatch → send reply)
define_counter!(EXPORT_COUNT);
define_counter!(EXPORT_RECV_NS);
define_counter!(EXPORT_DESER_NS);
define_counter!(EXPORT_DISPATCH_REPLY_NS);
define_counter!(EXPORT_REPLY_WRITE_NS);
define_counter!(EXPORT_TOTAL_NS);

// Import task (client-side: channel recv → serialize → send)
define_counter!(IMPORT_COUNT);
define_counter!(IMPORT_CHAN_RECV_NS);
define_counter!(IMPORT_REPLY_SETUP_NS);
define_counter!(IMPORT_SER_NS);
define_counter!(IMPORT_SEND_NS);
define_counter!(IMPORT_TOTAL_NS);

// Reply reader (client-side: recv reply → FIFO dequeue → deliver)
define_counter!(REPLY_COUNT);
define_counter!(REPLY_RECV_NS);
define_counter!(REPLY_FIFO_NS);
define_counter!(REPLY_DELIVER_NS);
define_counter!(REPLY_TOTAL_NS);

// Stream open/accept (one-time per actor)
define_counter!(OPEN_BI_COUNT);
define_counter!(OPEN_BI_NS);
define_counter!(ACCEPT_BI_COUNT);
define_counter!(ACCEPT_BI_NS);

// PreparedConn::get() — Shared<BoxFuture> clone+poll
define_counter!(CONN_GET_COUNT);
define_counter!(CONN_GET_NS);

fn print_phase(name: &str, total_ns: u64, count: u64) {
    if count > 0 {
        println!(
            "  {name:<20} total {:>10} µs, avg {:>8.1} µs",
            total_ns / 1000,
            total_ns as f64 / count as f64 / 1000.0
        );
    }
}

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

    // --- Control frame (legacy) ---
    println!("\n[send_control_frame] calls: {ctrl_count}");
    if ctrl_count > 0 {
        print_phase("serialize", ctrl_ser_ns, ctrl_count);
        print_phase("mutex_wait", ctrl_mutex_ns, ctrl_count);
        print_phase("write", ctrl_write_ns, ctrl_count);
        print_phase("total", ctrl_total_ns, ctrl_count);
    }

    // --- Raw write_all ---
    println!("\n[SendStream::send_frame] calls: {wf_count}");
    if wf_count > 0 {
        print_phase("len_prefix", wf_len_ns, wf_count);
        print_phase("data_write", wf_data_ns, wf_count);
    }

    // --- Export task (server) ---
    let ex_count = EXPORT_COUNT.load(Relaxed);
    let ex_recv = EXPORT_RECV_NS.load(Relaxed);
    let ex_deser = EXPORT_DESER_NS.load(Relaxed);
    let ex_dispatch = EXPORT_DISPATCH_REPLY_NS.load(Relaxed);
    let ex_reply_w = EXPORT_REPLY_WRITE_NS.load(Relaxed);
    let ex_total = EXPORT_TOTAL_NS.load(Relaxed);

    println!("\n[export_task] msgs: {ex_count}");
    if ex_count > 0 {
        print_phase("recv_frame", ex_recv, ex_count);
        print_phase("deserialize", ex_deser, ex_count);
        print_phase("dispatch+reply", ex_dispatch, ex_count);
        print_phase("reply_write", ex_reply_w, ex_count);
        print_phase("total", ex_total, ex_count);
        if ex_total > 0 {
            let accounted = ex_recv + ex_deser + ex_dispatch + ex_reply_w;
            let overhead = ex_total.saturating_sub(accounted);
            println!("  Breakdown (% of total):");
            println!("    recv_frame:     {:>5.1}%", ex_recv as f64 / ex_total as f64 * 100.0);
            println!("    deserialize:    {:>5.1}%", ex_deser as f64 / ex_total as f64 * 100.0);
            println!("    dispatch+reply: {:>5.1}%", ex_dispatch as f64 / ex_total as f64 * 100.0);
            println!("    reply_write:    {:>5.1}%", ex_reply_w as f64 / ex_total as f64 * 100.0);
            println!("    overhead:       {:>5.1}%", overhead as f64 / ex_total as f64 * 100.0);
        }
    }

    // --- Import task (client) ---
    let im_count = IMPORT_COUNT.load(Relaxed);
    let im_chan = IMPORT_CHAN_RECV_NS.load(Relaxed);
    let im_reply_setup = IMPORT_REPLY_SETUP_NS.load(Relaxed);
    let im_ser = IMPORT_SER_NS.load(Relaxed);
    let im_send = IMPORT_SEND_NS.load(Relaxed);
    let im_total = IMPORT_TOTAL_NS.load(Relaxed);

    println!("\n[import_task] msgs: {im_count}");
    if im_count > 0 {
        print_phase("chan_recv", im_chan, im_count);
        print_phase("reply_setup", im_reply_setup, im_count);
        print_phase("serialize", im_ser, im_count);
        print_phase("send_frame", im_send, im_count);
        print_phase("total", im_total, im_count);
        if im_total > 0 {
            let accounted = im_chan + im_reply_setup + im_ser + im_send;
            let overhead = im_total.saturating_sub(accounted);
            println!("  Breakdown (% of total):");
            println!("    chan_recv:     {:>5.1}%", im_chan as f64 / im_total as f64 * 100.0);
            println!("    reply_setup:  {:>5.1}%", im_reply_setup as f64 / im_total as f64 * 100.0);
            println!("    serialize:    {:>5.1}%", im_ser as f64 / im_total as f64 * 100.0);
            println!("    send_frame:   {:>5.1}%", im_send as f64 / im_total as f64 * 100.0);
            println!("    overhead:     {:>5.1}%", overhead as f64 / im_total as f64 * 100.0);
        }
    }

    // --- Reply reader (client) ---
    let rr_count = REPLY_COUNT.load(Relaxed);
    let rr_recv = REPLY_RECV_NS.load(Relaxed);
    let rr_fifo = REPLY_FIFO_NS.load(Relaxed);
    let rr_deliver = REPLY_DELIVER_NS.load(Relaxed);
    let rr_total = REPLY_TOTAL_NS.load(Relaxed);

    println!("\n[reply_reader] replies: {rr_count}");
    if rr_count > 0 {
        print_phase("recv_frame", rr_recv, rr_count);
        print_phase("fifo_dequeue", rr_fifo, rr_count);
        print_phase("deliver", rr_deliver, rr_count);
        print_phase("total", rr_total, rr_count);
    }

    // --- Stream open/accept ---
    let ob_count = OPEN_BI_COUNT.load(Relaxed);
    let ob_ns = OPEN_BI_NS.load(Relaxed);
    let ab_count = ACCEPT_BI_COUNT.load(Relaxed);
    let ab_ns = ACCEPT_BI_NS.load(Relaxed);

    println!("\n[bi_stream] open: {ob_count}, accept: {ab_count}");
    if ob_count > 0 {
        print_phase("open_bi", ob_ns, ob_count);
    }
    if ab_count > 0 {
        print_phase("accept_bi", ab_ns, ab_count);
    }

    // --- PreparedConn::get ---
    let cg_count = CONN_GET_COUNT.load(Relaxed);
    let cg_ns = CONN_GET_NS.load(Relaxed);
    println!("\n[conn_get] calls: {cg_count}");
    if cg_count > 0 {
        print_phase("clone+poll", cg_ns, cg_count);
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
    EXPORT_COUNT.store(0, Relaxed);
    EXPORT_RECV_NS.store(0, Relaxed);
    EXPORT_DESER_NS.store(0, Relaxed);
    EXPORT_DISPATCH_REPLY_NS.store(0, Relaxed);
    EXPORT_REPLY_WRITE_NS.store(0, Relaxed);
    EXPORT_TOTAL_NS.store(0, Relaxed);
    IMPORT_COUNT.store(0, Relaxed);
    IMPORT_CHAN_RECV_NS.store(0, Relaxed);
    IMPORT_REPLY_SETUP_NS.store(0, Relaxed);
    IMPORT_SER_NS.store(0, Relaxed);
    IMPORT_SEND_NS.store(0, Relaxed);
    IMPORT_TOTAL_NS.store(0, Relaxed);
    REPLY_COUNT.store(0, Relaxed);
    REPLY_RECV_NS.store(0, Relaxed);
    REPLY_FIFO_NS.store(0, Relaxed);
    REPLY_DELIVER_NS.store(0, Relaxed);
    REPLY_TOTAL_NS.store(0, Relaxed);
    OPEN_BI_COUNT.store(0, Relaxed);
    OPEN_BI_NS.store(0, Relaxed);
    ACCEPT_BI_COUNT.store(0, Relaxed);
    ACCEPT_BI_NS.store(0, Relaxed);
    CONN_GET_COUNT.store(0, Relaxed);
    CONN_GET_NS.store(0, Relaxed);
}
