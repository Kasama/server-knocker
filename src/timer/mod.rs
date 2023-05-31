use std::sync::Arc;
use std::time::Duration;

use log::debug;
use tokio::select;
use tokio::sync::Notify;
use tokio::task::JoinHandle;

pub struct ResetGuard {
    notify: Arc<Notify>,
}

impl ResetGuard {
    pub fn reset(&self) {
        self.notify.notify_one();
    }
}

pub struct ResetSignal {
    notifier: Arc<Notify>,
}

impl Default for ResetSignal {
    fn default() -> Self {
        Self {
            notifier: Arc::new(Notify::new()),
        }
    }
}

impl ResetSignal {
    pub fn get_guard(&self) -> ResetGuard {
        ResetGuard {
            notify: self.notifier.clone(),
        }
    }

    pub fn run_after(&self, duration: Duration) -> (ResetGuard, JoinHandle<()>) {
        let notifier = self.notifier.clone();
        let handle = tokio::spawn(async move {
            loop {
                select! {
                    _ = notifier.notified() => {
                        debug!("Resetting timer");
                    }
                    _ = tokio::time::sleep(duration) => {
                        debug!("Timer expired");
                        break;
                    }
                }
            }
        });
        let guard = self.get_guard();

        (guard, handle)
    }
}

#[cfg(test)]
mod test {
    use std::sync::atomic::AtomicU64;
    use std::sync::atomic::Ordering::Relaxed;
    use std::time::Duration;

    use tokio::select;
    use tokio::time::sleep;

    use crate::timer::ResetSignal;

    #[tokio::test]
    async fn does_expire() -> Result<(), anyhow::Error> {
        let resettable = ResetSignal::default();

        let (guard, mut handle) = resettable.run_after(Duration::from_millis(120));

        let time = AtomicU64::new(100);
        let pass = AtomicU64::new(1);

        loop {
            select! {
                _ = &mut handle => {
                    break;
                }
                _ = sleep(Duration::from_millis(time.load(Relaxed))) => {
                    let current_pass = pass.fetch_add(1, Relaxed);
                    time.fetch_add(50, Relaxed);
                    guard.reset();
                    assert!(current_pass < 2);
                }
            }
        }

        assert!(
            pass.load(Relaxed) >= 2,
            "ran loop less than 2 times: {:?}x",
            pass
        );
        Ok(())
    }
}
