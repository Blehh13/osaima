use zbus::{Connection, proxy};
use std::error::Error;

#[proxy(
    interface = "org.mpris.MediaPlayer2.Player",
    default_service = "org.mpris.MediaPlayer2.spotify", // Fallback, could search for others
    default_path = "/org/mpris/MediaPlayer2"
)]
trait MediaPlayer {
    fn play_pause(&self) -> zbus::Result<()>;
    fn next(&self) -> zbus::Result<()>;
    fn previous(&self) -> zbus::Result<()>;
    fn stop(&self) -> zbus::Result<()>;
}

#[proxy(
    interface = "org.freedesktop.Notifications",
    default_service = "org.freedesktop.Notifications",
    default_path = "/org/freedesktop/Notifications"
)]
trait Notifications {
    fn notify(
        &self,
        app_name: &str,
        replaces_id: u32,
        app_icon: &str,
        summary: &str,
        body: &str,
        actions: &[&str],
        hints: &std::collections::HashMap<&str, zbus::zvariant::Value<'_>>,
        expire_timeout: i32,
    ) -> zbus::Result<u32>;
}

pub struct DBusController {
    conn: Connection,
}

impl DBusController {
    pub async fn new() -> Result<Self, Box<dyn Error>> {
        let conn = Connection::session().await?;
        Ok(Self { conn })
    }

    /// Toggles Play/Pause for the active media player.
    pub async fn media_play_pause(&self, player: Option<&str>) -> Result<(), Box<dyn Error>> {
        let dest = match player {
            Some(p) => format!("org.mpris.MediaPlayer2.{}", p),
            None => "org.mpris.MediaPlayer2.spotify".to_string(), // Can be improved to find active player
        };
        let proxy = MediaPlayerProxy::builder(&self.conn)
            .destination(dest)?
            .build()
            .await?;
        proxy.play_pause().await?;
        Ok(())
    }

    pub async fn media_next(&self, player: Option<&str>) -> Result<(), Box<dyn Error>> {
        let dest = match player {
            Some(p) => format!("org.mpris.MediaPlayer2.{}", p),
            None => "org.mpris.MediaPlayer2.spotify".to_string(),
        };
        let proxy = MediaPlayerProxy::builder(&self.conn)
            .destination(dest)?
            .build()
            .await?;
        proxy.next().await?;
        Ok(())
    }

    pub async fn media_previous(&self, player: Option<&str>) -> Result<(), Box<dyn Error>> {
        let dest = match player {
            Some(p) => format!("org.mpris.MediaPlayer2.{}", p),
            None => "org.mpris.MediaPlayer2.spotify".to_string(),
        };
        let proxy = MediaPlayerProxy::builder(&self.conn)
            .destination(dest)?
            .build()
            .await?;
        proxy.previous().await?;
        Ok(())
    }

    /// Sends a desktop notification
    pub async fn send_notification(&self, title: &str, body: &str) -> Result<u32, Box<dyn Error>> {
        let proxy = NotificationsProxy::new(&self.conn).await?;
        let hints = std::collections::HashMap::new();
        let id = proxy.notify(
            "OSAIMA",
            0,
            "dialog-information",
            title,
            body,
            &[],
            &hints,
            5000,
        ).await?;
        Ok(id)
    }
}
