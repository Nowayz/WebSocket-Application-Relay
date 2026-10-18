
#[cfg(test)]
mod allocation_screen {
    use super::*;
    use std::{hint::black_box, io::{self, Write}, time::Instant};
    fn gate(marker: &str) {
        println!("{marker}");
        io::stdout().flush().unwrap();
        if std::env::var_os("PAGE_HEAP_GATES").is_some() {
            let mut line=String::new();io::stdin().read_line(&mut line).unwrap();assert_eq!(line,"go\n");
        }
    }
    #[test]
    #[ignore]
    fn management_page_screen() {
        let relay=Relay::new(crate::Config::default()).unwrap();
        let mut members=Vec::new();
        for i in 0..8192 {
            let mut sub=relay.subscribe(format!("m{i:08}").as_bytes()).unwrap();
            sub.outgoing.try_recv().unwrap();members.push(sub);
        }
        let all=relay.channel_listing();assert_eq!(all.len(),8193);
        let limit=NonZeroUsize::new(1000).unwrap();
        let tail=all[all.len()-17].name.base64();
        let empty=SnapshotName::new(&[0xcc;16]).base64();
        assert_eq!(serde_json::to_vec(&relay.channel_page(limit,None)).unwrap(),serde_json::to_vec(&all[..1000]).unwrap());
        assert_eq!(serde_json::to_vec(&relay.channel_page(limit,Some(tail))).unwrap(),serde_json::to_vec(&all[all.len()-16..]).unwrap());
        assert!(relay.channel_page(limit,Some(empty)).is_empty());
        gate("PAGE_READY");
        for (name,after,expected) in [("first",None,1000),("tail",Some(tail),16),("empty",Some(empty),0)] {
            for _ in 0..10 {black_box(relay.channel_page(limit,black_box(after)));}
            let mut times=Vec::new();
            for _ in 0..9 {
                let start=Instant::now();
                for _ in 0..100 {let page=relay.channel_page(limit,black_box(after));assert_eq!(page.len(),expected);black_box(page);}
                times.push(start.elapsed().as_nanos() as f64/100.0);
            }
            println!("{{\"case\":\"{name}\",\"operations_per_batch\":100,\"ns_per_operation\":{times:?}}}");
        }
        gate("PAGE_DONE");
        drop(members);assert_eq!(relay.counts(),(0,1));
    }
}
