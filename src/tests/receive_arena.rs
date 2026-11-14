use super::*;

fn clear() {
    PAGE.with(|p| *p.borrow_mut() = None);
    SPARE.with(|p| *p.borrow_mut() = None);
}

#[test]
fn retained_slices_cannot_be_overwritten_and_exhaustion_falls_back() {
    clear();
    let arena = ReceiveArena::new(128, 64);
    let mut first = arena.allocate(32).unwrap();
    first.as_mut().fill(1);
    let first = first.publish();
    let held = first.slice(3..9);
    drop(first);
    let mut second = arena.allocate(32).unwrap();
    second.as_mut().fill(2);
    let second = second.publish();
    let mut third = arena.allocate(64).unwrap();
    third.as_mut().fill(3);
    let third = third.publish();
    assert!(arena.allocate(1).is_none());
    assert_eq!(arena.budget.reserved(), 128);
    assert_eq!(&held[..], &[1; 6]);
    assert_eq!(&second[..], &[2; 32]);
    assert_eq!(&third[..], &[3; 64]);
    drop((held, second, third));
    clear();
    assert_eq!(arena.budget.reserved(), 0);
    assert!(arena.allocate(129).is_none());
    clear();
}

#[test]
fn consumed_page_is_reclaimed_without_growing_the_budget() {
    clear();
    let arena = ReceiveArena::new(64, 64);
    for n in 0..100 {
        let mut body = arena.allocate(17).unwrap();
        body.as_mut().fill(n);
        assert_eq!(&body.publish()[..], &[n; 17]);
    }
    assert_eq!(arena.budget.peak(), 64);
    assert_eq!(arena.budget.rejected(), 0);
    clear();
    assert_eq!(arena.budget.reserved(), 0);
    assert!(ReceiveArena::new(0, 64).allocate(1).is_none());
}

#[test]
fn last_slice_returns_a_completed_page_after_the_worker_moves_on() {
    clear();
    let arena = ReceiveArena::new(128, 64);
    let mut first = arena.allocate(32).unwrap();
    first.as_mut().fill(7);
    let first = first.publish();
    let second = arena.allocate(32).unwrap().publish();
    let third = arena.allocate(64).unwrap().publish();
    drop(first);
    assert_eq!(&second[..], &[0; 32]);
    drop(second);
    assert!(SPARE.with(|p| p.borrow().is_some()));
    assert_eq!(arena.budget.reserved(), 128);
    assert!(arena.allocate(64).is_some());
    assert_eq!(arena.budget.rejected(), 0);
    drop(third);
    clear();
    assert_eq!(arena.budget.reserved(), 0);
}
