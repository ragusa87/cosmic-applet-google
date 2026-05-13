use futures_util::{SinkExt, StreamExt};
use zbus::Connection;
use zbus::proxy;

#[derive(Debug, Clone, Copy)]
pub enum LockEvent {
    Locked,
    Unlocked,
}

#[proxy(
    interface = "org.freedesktop.ScreenSaver",
    default_service = "org.freedesktop.ScreenSaver",
    default_path = "/org/freedesktop/ScreenSaver"
)]
trait FreedesktopScreenSaver {
    #[zbus(signal)]
    fn active_changed(&self, active: bool) -> zbus::Result<()>;
}

#[proxy(
    interface = "org.freedesktop.login1.Manager",
    default_service = "org.freedesktop.login1",
    default_path = "/org/freedesktop/login1"
)]
trait Login1Manager {
    #[zbus(signal)]
    fn prepare_for_sleep(&self, start: bool) -> zbus::Result<()>;
}

pub fn stream() -> impl cosmic::iced::futures::Stream<Item = LockEvent> {
    cosmic::iced::stream::channel(8, |sender| async move {
        run(sender).await;
    })
}

async fn run(mut sender: cosmic::iced::futures::channel::mpsc::Sender<LockEvent>) {
    let screensaver_stream = match screensaver_subscription().await {
        Ok(s) => Some(s),
        Err(e) => {
            tracing::info!(error = %e, "no org.freedesktop.ScreenSaver subscription");
            None
        }
    };
    let sleep_stream = match sleep_subscription().await {
        Ok(s) => Some(s),
        Err(e) => {
            tracing::info!(error = %e, "no org.freedesktop.login1 subscription");
            None
        }
    };

    let mut merged = futures_util::stream::select_all(
        [screensaver_stream, sleep_stream]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>(),
    );

    let mut last_state: Option<bool> = None;
    while let Some(active) = merged.next().await {
        if Some(active) == last_state {
            continue;
        }
        last_state = Some(active);
        let event = if active {
            LockEvent::Locked
        } else {
            LockEvent::Unlocked
        };
        if sender.send(event).await.is_err() {
            break;
        }
    }
}

type BoolStream = std::pin::Pin<Box<dyn cosmic::iced::futures::Stream<Item = bool> + Send>>;

async fn screensaver_subscription() -> anyhow::Result<BoolStream> {
    let conn = Connection::session().await?;
    let proxy = FreedesktopScreenSaverProxy::new(&conn).await?;
    let stream = proxy
        .receive_active_changed()
        .await?
        .filter_map(|signal| async move {
            signal.args().ok().map(|a| a.active)
        });
    Ok(Box::pin(stream))
}

async fn sleep_subscription() -> anyhow::Result<BoolStream> {
    let conn = Connection::system().await?;
    let proxy = Login1ManagerProxy::new(&conn).await?;
    let stream = proxy
        .receive_prepare_for_sleep()
        .await?
        .filter_map(|signal| async move {
            signal.args().ok().map(|a| a.start)
        });
    Ok(Box::pin(stream))
}
