use actix_web::{HttpServer, App, web, Responder, HttpRequest, HttpResponse, Error};
use actix_ws::{Message, ProtocolError, CloseReason, MessageStream};
use std::collections::HashMap;
use tokio::{sync::mpsc, task::spawn_local, spawn};
use futures_util::{ StreamExt, FutureExt, try_join };
use merge_streams::MergeStreams;

struct UserSession {
    user_id: i32,
    ws_server_sender: mpsc::UnboundedSender<WebSocketCommands>,
    session: actix_ws::Session
}

enum UserSessionMessage {
    SessionMessage(Option<Result<Message, ProtocolError>>),
    ServerMessage(Option<String>)
}

impl UserSession {
    async fn handle_server_message(&mut self, msg: Option<String>) {
        let Some(msg) = msg else { return; };
        self.session.text(msg).await.unwrap();
    }

    async fn handle_session_message(&self, msg: Option<Result<Message, ProtocolError>>) -> Option<CloseReason> {
        use Message::*;
        use WebSocketCommands::*;
        let Some(msg) = msg else { return None; };
        let Ok(msg) = msg else { return None; };
        match msg {
            Text(text) => {
                self.ws_server_sender.send(UserMessage { user_id: self.user_id, user_message: text.to_string() }).unwrap();
                return None;
            },
            Close(reason) => return reason,
            _ => return None
        };
    }

    async fn run(mut self, mut msg_stream: MessageStream) {
        use WebSocketCommands::*;
        use UserSessionMessage::*;
        let (session_sender, mut session_receiver) = mpsc::unbounded_channel::<String>();
        let user_sender = session_sender;
        let user_id = self.user_id;
        self.ws_server_sender.send(UserConnect { user_id, user_sender }).unwrap();

        let reason = loop {

            let server_stream = session_receiver.recv().into_stream().map(ServerMessage);
            let session_stream = msg_stream.recv().into_stream().map(SessionMessage);
            let mut stream = (server_stream, session_stream).merge();
            let Some(msg) = stream.next().await else { continue; };
            match msg {
                SessionMessage(msg) => { 
                    if let Some(reason) = self.handle_session_message(msg).await { 
                        break reason;
                    };
                },
                ServerMessage(msg) => self.handle_server_message(msg).await
            };

        };
        self.session.close(Some(reason)).await.unwrap();
    }
}

#[derive(Clone)]
struct UserSessionFactory {
    ws_server_sender: mpsc::UnboundedSender<WebSocketCommands>
}

impl UserSessionFactory {
    fn create_session(
        &self, 
        user_id: i32, 
        session: actix_ws::Session
    ) -> UserSession {
        let ws_server_sender = self.ws_server_sender.clone();
        UserSession { user_id, ws_server_sender, session }
    }
}

type SessionStore = HashMap<i32, mpsc::UnboundedSender<String>>;
enum WebSocketCommands {
    UserConnect { user_id: i32, user_sender: mpsc::UnboundedSender<String> },
    UserMessage { user_id: i32, user_message: String }
}

struct WebSocketServer {
    session_store: SessionStore,
    ws_server_receiver: mpsc::UnboundedReceiver<WebSocketCommands>
}

impl WebSocketServer {

    fn new() -> (Self, UserSessionFactory) {
        let (ws_server_sender, ws_server_receiver) = mpsc::unbounded_channel::<WebSocketCommands>();
        let server = Self {
            session_store: SessionStore::new(),
            ws_server_receiver
        };
        let session_factory = UserSessionFactory {
            ws_server_sender
        };
        (server, session_factory)
    }

    async fn user_connect(
        &mut self,
        user_id: i32,
        user_sender: mpsc::UnboundedSender<String>
    ) {
        self.session_store.insert(user_id, user_sender);
    }

    async fn user_sends_message(
        &mut self,
        user_id: i32,
        message: String
    ) {
        for (_, user_sender) in self.session_store.iter() {
            user_sender.send(format!("[UID: {}]: {}", user_id, message.clone())).unwrap();
        }
    }

    async fn run(mut self) -> std::io::Result<()> {
        use WebSocketCommands::*;
        while let Some(msg) = self.ws_server_receiver.recv().await {
            match msg {
                UserConnect { user_id, user_sender } => self.user_connect(user_id, user_sender).await,
                UserMessage { user_id, user_message } => self.user_sends_message(user_id, user_message).await
            }
        }
        Ok(())
    }
}

pub fn get_uid_from_header(req: HttpRequest) -> Option<i32> {
    let uid = req
        .headers()
        .get("uid")
        .map(|v| v.to_str().ok())
        .flatten()
        .map(|s| s.to_string())
        .map(|s| s.parse::<i32>().ok())
        .flatten();
    uid
}

async fn chat_ws(
    req: HttpRequest,
    stream: web::Payload,
    session_factory: web::Data<UserSessionFactory>
) -> Result<HttpResponse, Error> {
    let (res, session, msg_stream) = actix_ws::handle(&req, stream)?;
    let Some(uid) = get_uid_from_header(req) else {
        session.close(Some(CloseReason { code: actix_ws::CloseCode::Error, description: Some("uid is missing".to_string()) })).await.unwrap();
        return Ok(res);
    };
    let session = session_factory.create_session(uid, session);
    spawn_local(session.run(msg_stream));

    Ok(res)
}

async fn healthcheck() -> impl Responder {
    "rust-websocket-tutorial is live"
}

#[actix_web::main]
async fn main() -> std::io::Result<()>{
    env_logger::init_from_env(env_logger::Env::new().default_filter_or("info"));

    let (websocket_server, session_factory) = WebSocketServer::new();

    let websocket_server = spawn(websocket_server.run());
    let port = 5000;

    log::info!("Server will be live on http://0.0.0.0:{port}");
    let http_server = HttpServer::new(move || {
        App::new()
            .app_data(web::Data::new(session_factory.clone()))
            .service(web::resource("/healthcheck").route(web::get().to(healthcheck)))
            .service(web::resource("/ws").route(web::get().to(chat_ws)))
    })
    .workers(2)
    .bind(("0.0.0.0", port))?
    .run();

    try_join!(http_server, async move { websocket_server.await.unwrap() })?;
    Ok(())
}
