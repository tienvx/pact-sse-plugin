use core::pin::Pin;
use core::task::{Context, Poll};
use std::io;
use std::io::Write;
use std::net::SocketAddr;

use env_logger::Env;
use futures::Stream;
use log::debug;
use maplit::hashmap;
use pact_models::matchingrules::{MatchingRule, RuleList, RuleLogic};
use pact_models::prelude::ContentType;
use serde_json::Value;
use tokio::net::{TcpListener, TcpStream};
use tonic::{Response, transport::Server};
use uuid::Uuid;

use crate::proto::body::ContentTypeHint;
use crate::proto::catalogue_entry::EntryType;
use crate::proto::pact_plugin_server::{PactPlugin, PactPluginServer};
use crate::sse_content::{compare_sse_contents, generate_sse_content, setup_sse_contents};

#[allow(dead_code)]
mod proto;
mod parser;
#[allow(dead_code)]
mod utils;
mod sse_content;

#[derive(Debug, Default)]
pub struct SsePactPlugin {}

#[tonic::async_trait]
impl PactPlugin for SsePactPlugin {
  async fn init_plugin(
    &self,
    request: tonic::Request<proto::InitPluginRequest>,
  ) -> Result<tonic::Response<proto::InitPluginResponse>, tonic::Status> {
    let message = request.get_ref();
    debug!("Init request from {}/{}", message.implementation, message.version);
    Ok(Response::new(proto::InitPluginResponse {
      catalogue: vec![
        proto::CatalogueEntry {
          r#type: EntryType::ContentMatcher as i32,
          key: "sse".to_string(),
          values: hashmap! {
            "content-types".to_string() => "text/event-stream".to_string()
          }
        },
        proto::CatalogueEntry {
          r#type: EntryType::ContentGenerator as i32,
          key: "sse".to_string(),
          values: hashmap! {
            "content-types".to_string() => "text/event-stream".to_string()
          }
        }
      ]
    }))
  }

  async fn update_catalogue(
    &self,
    _request: tonic::Request<proto::Catalogue>,
  ) -> Result<tonic::Response<()>, tonic::Status> {
    debug!("Update catalogue request, ignoring");
    Ok(Response::new(()))
  }

  async fn compare_contents(
    &self,
    request: tonic::Request<proto::CompareContentsRequest>,
  ) -> Result<tonic::Response<proto::CompareContentsResponse>, tonic::Status> {
    let request = request.get_ref();
    debug!("compare_contents request - {:?}", request);

    let rules = request.rules.iter()
      .map(|(key, rules)| {
        let rules = rules.rule.iter().fold(RuleList::empty(RuleLogic::And), |mut list, rule| {
          if let Value::Object(mut map) = crate::proto::to_object(rule.values.as_ref().unwrap()) {
            map.insert("match".to_string(), Value::String(rule.r#type.clone()));
            debug!("Creating matching rule with {:?}", map);
            list.add_rule(&MatchingRule::from_json(&Value::Object(map)).unwrap());
          }
          list
        });
        (key.clone(), rules)
      }).collect();

    match (request.expected.as_ref(), request.actual.as_ref()) {
      (Some(expected), Some(actual)) => {
        let expected_sse = std::str::from_utf8(expected.content.as_ref().unwrap())
          .map_err(|err| tonic::Status::aborted(format!("Failed to parse expected SSE: {}", err)))?;
        let actual_sse = std::str::from_utf8(actual.content.as_ref().unwrap())
          .map_err(|err| tonic::Status::aborted(format!("Failed to parse actual SSE: {}", err)))?;
        compare_sse_contents(expected_sse, actual_sse, request.allow_unexpected_keys, &rules)
          .map_err(|err| tonic::Status::aborted(format!("Failed to compare SSE contents: {}", err)))
      }
      (None, Some(actual)) => {
        let contents = actual.content.as_ref().unwrap();
        Ok(Response::new(proto::CompareContentsResponse {
          error: String::default(),
          type_mismatch: None,
          results: hashmap! {
            String::default() => proto::ContentMismatches {
              mismatches: vec![
                proto::ContentMismatch {
                  expected: None,
                  actual: Some(contents.clone()),
                  mismatch: format!("Expected no SSE content, but got {} bytes", contents.len()),
                  path: "".to_string(),
                  diff: "".to_string()
                }
              ]
            }
          }
        }))
      }
      (Some(expected), None) => {
        let contents = expected.content.as_ref().unwrap();
        Ok(Response::new(proto::CompareContentsResponse {
          error: String::default(),
          type_mismatch: None,
          results: hashmap! {
            String::default() => proto::ContentMismatches {
              mismatches: vec![
                proto::ContentMismatch {
                  expected: Some(contents.clone()),
                  actual: None,
                  mismatch: "Expected SSE content, but did not get any".to_string(),
                  path: "".to_string(),
                  diff: "".to_string()
                }
              ]
            }
          }
        }))
      }
      (None, None) => {
        Ok(Response::new(proto::CompareContentsResponse {
          error: String::default(),
          type_mismatch: None,
          results: hashmap!{}
        }))
      }
    }
  }

  async fn configure_interaction(
    &self,
    request: tonic::Request<proto::ConfigureInteractionRequest>,
  ) -> Result<tonic::Response<proto::ConfigureInteractionResponse>, tonic::Status> {
    debug!("Received configure_contents request for '{}'", request.get_ref().content_type);
    setup_sse_contents(&request)
      .map_err(|err| tonic::Status::aborted(format!("Invalid SSE definition: {}", err)))
  }

  async fn generate_content(
    &self,
    request: tonic::Request<proto::GenerateContentRequest>,
  ) -> Result<tonic::Response<proto::GenerateContentResponse>, tonic::Status> {
    debug!("Received generate_content request");
    generate_sse_content(&request)
      .map(|contents| {
        debug!("Generated contents: {}", contents);
        Response::new(proto::GenerateContentResponse {
          contents: Some(proto::Body {
            content_type: contents.content_type().unwrap_or(ContentType::from("text/event-stream")).to_string(),
            content: Some(contents.value().unwrap().to_vec()),
            content_type_hint: ContentTypeHint::Default as i32
          })
        })
      })
      .map_err(|err| tonic::Status::aborted(format!("Failed to generate SSE contents: {}", err)))
  }
}

struct TcpIncoming {
  inner: TcpListener
}

impl Stream for TcpIncoming {
  type Item = Result<TcpStream, std::io::Error>;

  fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
    Pin::new(&mut self.inner).poll_accept(cx)
      .map_ok(|(stream, _)| stream).map(Some)
  }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
  let env = Env::new().filter("LOG_LEVEL");
  env_logger::init_from_env(env);

  let addr: SocketAddr = "0.0.0.0:0".parse()?;
  let listener = TcpListener::bind(addr).await?;
  let address = listener.local_addr()?;

  let server_key = Uuid::new_v4().to_string();
  println!("{{\"port\":{}, \"serverKey\":\"{}\"}}", address.port(), server_key);
  let _ = io::stdout().flush();

  let plugin = SsePactPlugin::default();
  Server::builder()
    .add_service(PactPluginServer::new(plugin))
    .serve_with_incoming(TcpIncoming { inner: listener }).await?;

  Ok(())
}
