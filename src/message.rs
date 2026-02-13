use std::{
    process::Stdio,
    time::{Duration, SystemTime},
};

use matrix_sdk::{
    Client, Room, RoomState,
    ruma::events::room::{
        ImageInfo,
        message::{
            AddMentions, ForwardThread, ImageMessageEventContent, MessageType,
            OriginalSyncRoomMessageEvent, RoomMessageEventContent,
        },
    },
};
use mime::IMAGE_PNG;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    time::timeout,
};

const PREAMBLE: &str = r#"
#import "@preview/catppuccin:1.0.0": catppuccin, flavors;
#show: catppuccin.with(flavors.mocha);
#set page(height: auto, width: auto, margin: 28pt);
#set text(size: 44pt);
"#;

/// Handle room messages.
pub async fn on_room_message(event: OriginalSyncRoomMessageEvent, room: Room, client: Client) {
    // We only want to log text messages in joined rooms.
    if room.state() != RoomState::Joined {
        return;
    }

    if SystemTime::now()
        .duration_since(event.origin_server_ts.to_system_time().unwrap())
        .unwrap()
        >= Duration::from_secs(5)
    {
        return;
    }

    let MessageType::Text(text_content) = &event.content.msgtype else {
        return;
    };

    let Some(content) = text_content.body.strip_prefix(",typ") else {
        return;
    };

    let reply = if content.trim().is_empty() {
        RoomMessageEventContent::text_plain("<text> is needed to typeset").make_reply_to(
            &event,
            ForwardThread::Yes,
            AddMentions::Yes,
        )
    } else {
        let mut child = tokio::process::Command::new("typst")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .args(["compile", "-", "-", "--format", "png"])
            .spawn()
            .unwrap();

        let mut stdin = child.stdin.take().unwrap();
        stdin
            .write_all(format!("{PREAMBLE}\n{content}").as_bytes())
            .await
            .unwrap();
        drop(stdin);

        let mut buf = vec![];

        let mut stdout = child.stdout.take().unwrap();
        if timeout(Duration::from_secs(25), stdout.read_to_end(&mut buf))
            .await
            .is_err()
        {
            room.send(
                RoomMessageEventContent::text_plain("Your code took too long (>10s) to render")
                    .make_reply_to(&event, ForwardThread::Yes, AddMentions::Yes),
            )
            .await
            .unwrap();

            return;
        };

        let mut stderr = child.stderr.take().unwrap();
        stderr.read_to_end(&mut buf).await.unwrap();

        let stat = child.wait().await.unwrap();

        let msg = if !stat.success() {
            let err = String::from_utf8_lossy(&buf).into_owned();
            let html_text = format!(
                "<pre><code class=\"language-typst\">{}</code></pre>",
                html_escape::encode_safe(&err)
            );

            MessageType::text_html(err, html_text)
        } else {
            let img = image::load_from_memory(&buf).unwrap();
            let (width, height) = (img.width(), img.height());

            let response = client.media().upload(&IMAGE_PNG, buf, None).await.unwrap();

            let mut info = ImageInfo::new();

            info.height = Some(height.into());
            info.width = Some(width.into());

            MessageType::Image(
                ImageMessageEventContent::plain(String::new(), response.content_uri)
                    .info(Some(Box::new(info))),
            )
        };

        RoomMessageEventContent::new(msg).make_reply_to(
            &event,
            ForwardThread::Yes,
            AddMentions::Yes,
        )
    };

    room.send(reply).await.unwrap();
}
