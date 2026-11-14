use super::*;

#[test]
fn leases_reserve_until_dropped_and_refusals_are_counted() {
    let budget = ReadBudget::new(10);
    let first = budget.try_lease(6).unwrap();
    assert!(budget.try_lease(5).is_err());
    let second = budget.try_lease(4).unwrap();
    assert_eq!(
        (budget.reserved(), budget.peak(), budget.rejected()),
        (10, 10, 1)
    );
    drop(first);
    drop(second);
    assert_eq!(budget.reserved(), 0);
    assert!(budget.try_lease(0).is_ok());
    assert!(ReadBudget::new(0).try_lease(1).is_err());
}
