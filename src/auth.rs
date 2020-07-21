use actix_web::web::{self, HttpResponse};
use actix_web::{get, Scope, dev::ServiceRequest, dev::ServiceResponse};
use actix_web::error::{Error, ResponseError};
use actix_web::http::header;
use actix_service::ServiceFactory;
use actix_session::Session;
use oauth2::basic::BasicClient;
use oauth2::reqwest::async_http_client;
use oauth2::http::{HeaderMap, Method, header::AUTHORIZATION};
use oauth2::url::Url;
use reqwest;
use serde::Deserialize;
use oauth2::{
    AuthUrl, AuthorizationCode, ClientId, ClientSecret, CsrfToken, PkceCodeChallenge,
    RedirectUrl, Scope as OAuthScope, TokenResponse, TokenUrl, AsyncCodeTokenRequest,
};
use crate::cfg::Config;

struct AuthState {
    oauth_client: BasicClient,
    oauth_base_url: String
}

#[derive(Deserialize)]
struct AuthRequest {
    code: String,
    state: String,
}

#[derive(Debug)]
struct RequestTokenError {
    source: oauth2::RequestTokenError<oauth2::reqwest::Error<reqwest::Error>, oauth2::StandardErrorResponse<oauth2::basic::BasicErrorResponseType>>
}

impl core::fmt::Display for RequestTokenError {
    fn fmt(&self, f: &mut core::fmt::Formatter) -> core::fmt::Result {
        self.source.fmt(f)
    }
}

impl ResponseError for RequestTokenError {
}

#[get("/gitlab/login")]
fn login(data: web::Data<AuthState>) -> HttpResponse {
    let (pkce_code_challenge, _pkce_code_verifier) = PkceCodeChallenge::new_random_sha256();
    let (auth_url, _csrf_token) = &data
        .oauth_client
        .authorize_url(CsrfToken::new_random)
        .add_scope(OAuthScope::new("openid".to_string()))
        .set_pkce_challenge(pkce_code_challenge)
        .url();

    HttpResponse::Found()
        .header(header::LOCATION, auth_url.to_string())
        .finish()
}

#[get("/logout")]
fn logout(session: Session) -> HttpResponse {
    session.remove("login");
    HttpResponse::Found()
        .header(header::LOCATION, "/".to_string())
        .finish()
}

#[derive(Deserialize, Debug)]
struct UserInfo {
    nickname: String
}

async fn get_username(base_url: &str, secret: &str) -> String {
    let mut headers = HeaderMap::new();
    headers.insert(AUTHORIZATION, format!("Bearer {}", secret).parse().unwrap());
    let response = async_http_client(oauth2::HttpRequest {
        url: Url::parse(&format!("{}/oauth/userinfo", base_url)).unwrap(),
        method: Method::GET,
        headers,
        body: vec![]
    }).await.unwrap();
    serde_json::from_slice::<UserInfo>(&response.body).unwrap().nickname
}

#[get("/gitlab")]
async fn auth(session: Session, data: web::Data<AuthState>, params: web::Query<AuthRequest>) -> Result<HttpResponse, Error> {
    let code = AuthorizationCode::new(params.code.clone());
    let _state = CsrfToken::new(params.state.clone());

    // Exchange the code with a token.
    let token = &data
        .oauth_client
        .exchange_code(code)
        .request_async(async_http_client)
        .await.map_err(|e| RequestTokenError { source: e })?;

    let username = get_username(&data.oauth_base_url, token.access_token().secret()).await;
    session.set("login", username).unwrap();

    Ok(HttpResponse::Found()
        .header(header::LOCATION, "/".to_string())
        .finish())
}

pub fn scope(config: &Config) -> Scope<impl ServiceFactory<Config=(), InitError=(), Error=Error, Response=ServiceResponse, Request=ServiceRequest>> {
    let gitlab_config = config.auth.gitlab.as_ref().unwrap();
    let oauth_client = BasicClient::new(
        ClientId::new(gitlab_config.app_id.clone()),
        Some(ClientSecret::new(gitlab_config.secret.clone())),
        AuthUrl::new(format!("{}/oauth/authorize", gitlab_config.base_url)).unwrap(),
        Some(TokenUrl::new(format!("{}/oauth/token", gitlab_config.base_url)).unwrap())
    ).set_redirect_url(
        RedirectUrl::new(format!("{}/auth/gitlab", config.base_url)).unwrap()
    );
    // TODO: Fix signing secret
    Scope::new("/auth")
        .data(AuthState { oauth_client, oauth_base_url: gitlab_config.base_url.clone() })
        .service(login).service(logout).service(auth)
}
