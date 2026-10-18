#[cfg(test)]
#[test]
fn metrics_cache_line_layout_screen() {
    let m = Arc::new(Metrics::default());
    let base = Arc::as_ptr(&m) as usize;
    println!("METRICS_LAYOUT size={} align={} base_mod64={} incoming={} delivery={} bytes={} queue={}", std::mem::size_of::<Metrics>(), std::mem::align_of::<Metrics>(), base % 64, std::ptr::addr_of!(m.incoming_messages) as usize - base, std::ptr::addr_of!(m.deliveries) as usize - base, std::ptr::addr_of!(m.delivered_bytes) as usize - base, std::ptr::addr_of!(m.queued_bytes) as usize - base);
}
