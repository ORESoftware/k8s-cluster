#!/usr/bin/env python3
"""Apply reviewed compile-safety repairs to the one-shot fleet generator."""
from __future__ import annotations

import sys
from pathlib import Path


def replace_once(text: str, old: str, new: str, label: str) -> str:
    count = text.count(old)
    if count != 1:
        raise SystemExit(f"{label}: expected exactly one match, found {count}")
    return text.replace(old, new, 1)


def main() -> int:
    if len(sys.argv) != 2:
        raise SystemExit("usage: repair_four_org_generator.py PATH")
    path = Path(sys.argv[1])
    text = path.read_text()

    text = replace_once(
        text,
        '                assert_eq!(value, {rust_literal(product.statuses[0])});',
        '                assert_eq!(value, serde_json::to_string(&{rust_literal(product.statuses[0])}).unwrap());',
        "wire-value serialization assertion",
    )

    text = replace_once(
        text,
        '''        async fn socket_loop(mut socket: WebSocket, mut events: broadcast::Receiver<String>) {{
            loop {{
                tokio::select! {{
                    event = events.recv() => match event {{
                        Ok(text) if socket.send(Message::Text(text.into())).await.is_err() => break,
                        Err(broadcast::error::RecvError::Closed) => break,
                        _ => {{}},
                    }},
                    message = socket.next() => match message {{
                        Some(Ok(Message::Ping(data))) => {{ if socket.send(Message::Pong(data)).await.is_err() {{ break; }} }},
                        Some(Ok(Message::Close(_))) | None => break,
                        Some(Err(_)) => break,
                        _ => {{}},
                    }}
                }}
            }}
        }}''',
        '''        async fn socket_loop(socket: WebSocket, mut events: broadcast::Receiver<String>) {{
            let (mut sender, mut receiver) = socket.split();
            loop {{
                tokio::select! {{
                    event = events.recv() => match event {{
                        Ok(text) => {{
                            if sender.send(Message::Text(text.into())).await.is_err() {{ break; }}
                        }},
                        Err(broadcast::error::RecvError::Closed) => break,
                        _ => {{}},
                    }},
                    message = receiver.next() => match message {{
                        Some(Ok(Message::Ping(data))) => {{
                            if sender.send(Message::Pong(data)).await.is_err() {{ break; }}
                        }},
                        Some(Ok(Message::Close(_))) | None => break,
                        Some(Err(_)) => break,
                        _ => {{}},
                    }}
                }}
            }}
        }}''',
        "Axum API WebSocket split",
    )

    text = replace_once(
        text,
        '''        async fn ws_loop(mut socket: WebSocket, mut events: broadcast::Receiver<String>) {
            loop {
                tokio::select! {
                    event = events.recv() => match event {
                        Ok(text) if socket.send(Message::Text(text.into())).await.is_err() => break,
                        Err(broadcast::error::RecvError::Closed) => break,
                        _ => {},
                    },
                    incoming = socket.next() => match incoming {
                        Some(Ok(Message::Ping(data))) => { if socket.send(Message::Pong(data)).await.is_err() { break; } },
                        Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                        _ => {},
                    }
                }
            }
        }''',
        '''        async fn ws_loop(socket: WebSocket, mut events: broadcast::Receiver<String>) {
            let (mut sender, mut receiver) = socket.split();
            loop {
                tokio::select! {
                    event = events.recv() => match event {
                        Ok(text) => {
                            if sender.send(Message::Text(text.into())).await.is_err() { break; }
                        },
                        Err(broadcast::error::RecvError::Closed) => break,
                        _ => {},
                    },
                    incoming = receiver.next() => match incoming {
                        Some(Ok(Message::Ping(data))) => {
                            if sender.send(Message::Pong(data)).await.is_err() { break; }
                        },
                        Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                        _ => {},
                    }
                }
            }
        }''',
        "shared WebSocket split",
    )

    mash_start = text.index("def mash_main")
    mash_end = text.index("def generate_mash", mash_start)
    mash = replace_once(
        text[mash_start:mash_end],
        "            routing::get,",
        "            routing::{{get, post}},",
        "MASH post router import",
    )
    text = text[:mash_start] + mash + text[mash_end:]

    text = replace_once(
        text,
        '            let _ = state.events.send("{{\\"event_type\\":\\"record.changed\\"}}".into());',
        '            let _ = state.events.send(serde_json::json!({{"event_type":"record.changed"}}).to_string());',
        "MASH JSON event",
    )

    text = replace_once(
        text,
        '''        let body = leptos::ssr::render_to_string(|| view! {{
            <main>
                <h1>{rust_literal(product.title)}</h1>
                <p>{rust_literal(product.tagline)}</p>
                <button id="emit">"Emit demo WebSocket event"</button>
                <pre id="events">"waiting for events"</pre>
            </main>
        }});''',
        '''        let body = view! {{
            <main>
                <h1>{rust_literal(product.title)}</h1>
                <p>{rust_literal(product.tagline)}</p>
                <button id="emit">"Emit demo WebSocket event"</button>
                <pre id="events">"waiting for events"</pre>
            </main>
        }}.to_html();''',
        "Leptos 0.7 RenderHtml path",
    )

    path.write_text(text)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
