use super::*;

fn held(after: Duration, term: u64) -> Option<HeldUntil> {
    Some(HeldUntil::new(Instant::now() + after, term))
}

#[tokio::test]
async fn a_fence_is_open_until_its_deadline_and_an_open_one_stays_open() {
    assert!(CollectionFence::open().is_open());
    let (hold, receiver) = watch::channel(held(Duration::from_millis(50), 0));
    let fence = CollectionFence::following(receiver);
    assert!(fence.is_open());
    // Closed by the clock, though nothing moved the deadline.
    tokio::time::sleep(Duration::from_millis(60)).await;
    assert!(!fence.is_open());
    hold.send(held(Duration::from_secs(60), 0)).unwrap();
    assert!(fence.is_open());
    hold.send(None).unwrap();
    assert!(!fence.is_open());
}

#[tokio::test]
async fn a_bounded_step_is_refused_while_closed_and_abandoned_at_the_deadline() {
    let (hold, receiver) = watch::channel(None);
    let fence = CollectionFence::following(receiver);
    assert_eq!(fence.bounded(async { 1 }).await, None);
    hold.send(held(Duration::from_millis(50), 0)).unwrap();
    assert_eq!(fence.bounded(async { 2 }).await, Some(2));
    let started = Instant::now();
    assert_eq!(fence.bounded(std::future::pending::<()>()).await, None);
    assert!(started.elapsed() < Duration::from_secs(1));
    assert_eq!(CollectionFence::open().bounded(async { 3 }).await, Some(3));
}

/// A renewal that lands while a step runs extends the step's bound with it; a retake, a new term, does not.
#[tokio::test]
async fn a_bounded_step_follows_its_lease_renewed_and_not_retaken() {
    let (hold, receiver) = watch::channel(held(Duration::from_millis(40), 0));
    let fence = CollectionFence::following(receiver);
    let renewing = async {
        for _ in 0..4 {
            tokio::time::sleep(Duration::from_millis(25)).await;
            hold.send(held(Duration::from_millis(40), 0)).unwrap();
        }
    };
    let slow = tokio::time::sleep(Duration::from_millis(90));
    let (finished, ()) = tokio::join!(fence.bounded(slow), renewing);
    assert_eq!(
        finished,
        Some(()),
        "a step past the first deadline, renewed, was abandoned"
    );

    hold.send(held(Duration::from_millis(30), 0)).unwrap();
    let retaking = async {
        tokio::time::sleep(Duration::from_millis(20)).await;
        hold.send(held(Duration::from_secs(60), 1)).unwrap();
    };
    let (finished, ()) = tokio::join!(fence.bounded(std::future::pending::<()>()), retaking);
    assert_eq!(finished, None, "a step outlived its term");
}

/// A lease lost and taken again is a new term: held through the old one it was not.
#[test]
fn a_retaken_lease_is_a_new_term() {
    let (hold, receiver) = watch::channel(held(Duration::from_secs(60), 3));
    let fence = CollectionFence::following(receiver);
    let term = fence.term().expect("open");
    assert!(fence.held_through(term));
    hold.send(None).unwrap();
    assert!(!fence.held_through(term));
    hold.send(held(Duration::from_secs(60), 4)).unwrap();
    assert!(fence.is_open());
    assert!(!fence.held_through(term));
    assert_eq!(CollectionFence::open().term(), Some(0));
}

/// A pass is entered only while the fence is open, and leaves on every way out: dropped, returned early from,
/// or unwound by a panic.
#[tokio::test]
async fn a_pass_runs_while_entered_and_leaves_on_every_way_out() {
    let (hold, receiver) = watch::channel(None);
    let fence = CollectionFence::following(receiver);
    assert!(fence.enter("closed").is_none());
    assert!(fence.status().running.is_empty());

    hold.send(held(Duration::from_secs(60), 0)).unwrap();
    let pass = fence.enter("sweep").expect("open");
    let status = fence.status();
    assert!(status.open);
    assert_eq!(
        status
            .running
            .iter()
            .map(|pass| pass.name)
            .collect::<Vec<_>>(),
        ["sweep"]
    );
    drop(pass);
    assert!(fence.status().running.is_empty());

    fn early(fence: &CollectionFence) -> Option<()> {
        let _pass = fence.enter("early")?;
        None?;
        Some(())
    }
    assert_eq!(early(&fence), None);
    assert!(
        fence.status().running.is_empty(),
        "an early return kept its pass"
    );

    let panicking = fence.clone();
    let unwound = tokio::spawn(async move {
        let _pass = panicking.enter("panics").expect("open");
        panic!("the pass panics");
    })
    .await;
    assert!(unwound.is_err());
    assert!(fence.status().running.is_empty(), "a panic kept its pass");
}
