mod message;

use std::{
    env,
    path::Path,
    time::Duration,
};

use matrix_sdk::{
    Client, Error, LoopCtrl, Room,
    authentication::matrix::MatrixSession,
    config::SyncSettings,
    ruma::{
        api::client::filter::FilterDefinition,
        events::room::member::StrippedRoomMemberEvent,
    },
};
use serde::{Deserialize, Serialize};
use tokio::fs::{self};

/// The full session to persist.
#[derive(Debug, Serialize, Deserialize)]
struct FullSession {
    user_session: MatrixSession,
    /// The latest sync token.
    #[serde(skip_serializing_if = "Option::is_none")]
    sync_token: Option<String>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv()?;
    let session_file = env::var("SESSION_FILE").unwrap();

    let (client, sync_token) = if Path::exists(session_file.as_ref()) {
        restore_session(session_file.as_ref()).await?
    } else {
        (login(session_file.as_ref()).await?, None)
    };

    sync(client, sync_token, session_file.as_ref()).await
}

async fn restore_session(session_file: &Path) -> anyhow::Result<(Client, Option<String>)> {
    println!(
        "Previous session found in '{}'",
        session_file.to_string_lossy()
    );

    // The session was serialized as JSON in a file.
    let serialized_session = fs::read_to_string(&env::var("SESSION_FILE")?).await?;
    let FullSession {
        user_session,
        sync_token,
    } = serde_json::from_str(&serialized_session)?;

    // Build the client with the previous settings from the session.
    let client = Client::builder()
        .homeserver_url(&env::var("HOMESERVER")?)
        .sqlite_store(&env::var("DB_DIR")?, None)
        .build()
        .await?;

    println!("Restoring session for {}…", user_session.meta.user_id);

    // Restore the Matrix user session.
    client.restore_session(user_session).await?;

    Ok((client, sync_token))
}

async fn login(session_file: &Path) -> anyhow::Result<Client> {
    println!("No previous session found, logging in…");

    let client = build_client().await?;
    let matrix_auth = client.matrix_auth();
    let username = env::var("USERNAME")?;

    matrix_auth
        .login_username(&username, &env::var("PASSWORD")?)
        .initial_device_display_name(&username)
        .await?;

    let user_session = matrix_auth
        .session()
        .expect("A logged-in client should have a session");

    let serialized_session = serde_json::to_string(&FullSession {
        user_session,
        sync_token: None,
    })?;

    fs::write(session_file, serialized_session).await?;

    println!("Session persisted in {}", session_file.to_string_lossy());

    // After logging in, you might want to verify this session with another one (see
    // the `emoji_verification` example), or bootstrap cross-signing if this is your
    // first session with encryption, or if you need to reset cross-signing because
    // you don't have access to your old sessions (see the
    // `cross_signing_bootstrap` example).

    Ok(client)
}

/// Build a new client.
async fn build_client() -> anyhow::Result<Client> {
    match Client::builder()
        .homeserver_url(env::var("HOMESERVER")?)
        .sqlite_store(env::var("DB_DIR")?, None)
        .build()
        .await
    {
        Ok(client) => Ok(client),
        Err(error) => match &error {
            matrix_sdk::ClientBuildError::AutoDiscovery(_)
            | matrix_sdk::ClientBuildError::Url(_)
            | matrix_sdk::ClientBuildError::Http(_) => {
                println!("Error checking the homeserver: {error}");
                println!("Please try again\n");

                std::process::exit(1);
            }
            _ => {
                // Forward other errors, it's unlikely we can retry with a different outcome.
                Err(error.into())
            }
        },
    }
}

/// Setup the client to listen to new messages.
async fn sync(
    client: Client,
    initial_sync_token: Option<String>,
    session_file: &Path,
) -> anyhow::Result<()> {
    println!("Launching a first sync to ignore past messages…");

    let filter = FilterDefinition::with_lazy_loading();

    let mut sync_settings = SyncSettings::default().filter(filter.into());

    if let Some(sync_token) = initial_sync_token {
        sync_settings = sync_settings.token(sync_token);
    }

    loop {
        match client.sync_once(sync_settings.clone()).await {
            Ok(response) => {
                // This is the last time we need to provide this token, the sync method after
                // will handle it on its own.
                sync_settings = sync_settings.token(response.next_batch.clone());
                persist_sync_token(session_file, response.next_batch).await?;
                break;
            }
            Err(error) => {
                println!("An error occurred during initial sync: {error}");
                println!("Trying again…");
            }
        }
    }

    println!("The client is ready! Listening to new messages…");

    client.add_event_handler(message::on_room_message);
    client.add_event_handler(on_stripped_member);

    client
        .sync_with_result_callback(sync_settings, |sync_result| async move {
            let response = sync_result?;

            // We persist the token each time to be able to restore our session
            persist_sync_token(session_file, response.next_batch)
                .await
                .map_err(|err| Error::UnknownError(err.into()))?;

            Ok(LoopCtrl::Continue)
        })
        .await?;

    Ok(())
}

async fn persist_sync_token(session_file: &Path, sync_token: String) -> anyhow::Result<()> {
    let serialized_session = fs::read_to_string(session_file).await?;
    let mut full_session: FullSession = serde_json::from_str(&serialized_session)?;

    full_session.sync_token = Some(sync_token);
    let serialized_session = serde_json::to_string(&full_session)?;
    fs::write(session_file, serialized_session).await?;

    Ok(())
}

async fn on_stripped_member(room_member: StrippedRoomMemberEvent, client: Client, room: Room) {
    if room_member.state_key != client.user_id().unwrap() {
        return;
    }

    tokio::spawn(async move {
        let mut delay = 2;

        while let Err(err) = room.join().await {
            tokio::time::sleep(Duration::from_secs(delay)).await;
            delay *= 2;

            if delay >= 3600 {
                eprintln!("Can't join room {} ({err:?})", room.room_id());
                break;
            }
        }
    });
}
