//! The place a task holds among a limited number while its work runs, and gives up while the work only
//! waits for the node, such as a deposit's credit or a withdrawal's payout: the task running the work
//! names its place, and a wait that goes through `wait` frees the place for the length of the wait and
//! takes one again after it. A task with no place just sleeps.

use std::future::Future;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio::time::sleep;

/// One of the places a semaphore holds, taken for a task.
pub struct Place {
    places: Arc<Semaphore>,
    held: Mutex<Option<OwnedSemaphorePermit>>,
}

tokio::task_local! {
    static PLACE: Arc<Place>;
}

impl Place {
    /// Waits for one of `places`, which is held until the returned place is dropped or given up.
    pub async fn take(places: Arc<Semaphore>) -> Arc<Self> {
        let permit = Arc::clone(&places).acquire_owned().await.expect("the semaphore is never closed");
        Arc::new(Self { places, held: Mutex::new(Some(permit)) })
    }

    fn held(&self) -> MutexGuard<'_, Option<OwnedSemaphorePermit>> {
        self.held.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// Runs `work` as the task of `place`.
pub async fn with_place<F: Future>(place: Arc<Place>, work: F) -> F::Output {
    PLACE.scope(place, work).await
}

/// Sleeps for `duration`, and while it does gives up the task's place, if it has one, so another task
/// runs in it; afterwards it waits for a place again, behind those already waiting.
pub async fn wait(duration: Duration) {
    let Ok(place) = PLACE.try_with(Arc::clone) else {
        return sleep(duration).await;
    };
    drop(place.held().take());
    sleep(duration).await;
    let permit = Arc::clone(&place.places).acquire_owned().await.expect("the semaphore is never closed");
    *place.held() = Some(permit);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A task that waits frees its place for another, and takes one again after the wait.
    #[tokio::test(start_paused = true)]
    async fn a_wait_frees_the_place_for_its_length() {
        let places = Arc::new(Semaphore::new(1));
        let waiting = Place::take(Arc::clone(&places)).await;
        let sleeper = tokio::spawn(with_place(waiting, wait(Duration::from_secs(10))));
        tokio::task::yield_now().await;
        // The place is free during the wait.
        let other = tokio::time::timeout(Duration::from_secs(1), Place::take(Arc::clone(&places))).await;
        assert!(other.is_ok(), "the waiting task gave its place up");
        let other = other.unwrap();
        // The waiter takes a place again only once the other frees one.
        tokio::time::sleep(Duration::from_secs(20)).await;
        assert!(!sleeper.is_finished(), "it waits for a place after its wait");
        drop(other);
        sleeper.await.unwrap();
        assert_eq!(places.available_permits(), 1, "its place is given back when it ends");
    }

    /// With no place, `wait` is a sleep.
    #[tokio::test(start_paused = true)]
    async fn a_task_with_no_place_just_sleeps() {
        let start = tokio::time::Instant::now();
        wait(Duration::from_secs(5)).await;
        assert_eq!(start.elapsed(), Duration::from_secs(5));
    }
}
