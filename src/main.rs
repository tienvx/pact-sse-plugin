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
use tonic::{transport::Server, Response};
use uuid::Uuid;

use crate::proto::body::ContentTypeHint;
use crate::proto::catalogue_entry::EntryType;
use crate::proto::init_plugin_response::Response as InitResponse;
use crate::proto::pact_plugin_server::{PactPlugin, PactPluginServer};
use crate::proto::start_mock_server_response::Response as MockServerResponse;
use crate::proto::verification_preparation_response::Response as PrepResponse;
use crate::proto::verify_interaction_response::Response as VerifyResponse;
use crate::sse_content::{compare_sse_contents, generate_sse_content, setup_sse_contents};

mod parser;
#[allow(dead_code)]
mod proto;
mod sse_content;
#[allow(dead_code)]
mod utils;

#[derive(Debug, Default)]
pub struct SsePactPlugin {}

#[tonic::async_trait]
impl PactPlugin for SsePactPlugin {
    async fn init_plugin(
        &self,
        request: tonic::Request<proto::InitPluginRequest>,
    ) -> Result<tonic::Response<proto::InitPluginResponse>, tonic::Status> {
        let message = request.get_ref();
        debug!(
            "Init request from {}/{}, host capabilities: {:?}",
            message.implementation, message.version, message.host_capabilities
        );
        Ok(Response::new(proto::InitPluginResponse {
            response: Some(InitResponse::Success(proto::InitPluginSuccess {
                catalogue: vec![
                    proto::CatalogueEntry {
                        r#type: EntryType::ContentMatcher as i32,
                        key: "sse".to_string(),
                        values: hashmap! {
                          "content-types".to_string() => "text/event-stream".to_string()
                        },
                    },
                    proto::CatalogueEntry {
                        r#type: EntryType::ContentGenerator as i32,
                        key: "sse".to_string(),
                        values: hashmap! {
                          "content-types".to_string() => "text/event-stream".to_string()
                        },
                    },
                ],
                plugin_capabilities: vec![],
            })),
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

        let rules = request
            .rules
            .iter()
            .map(|(key, rules)| {
                let rules =
                    rules
                        .rule
                        .iter()
                        .fold(RuleList::empty(RuleLogic::And), |mut list, rule| {
                            if let Value::Object(mut map) =
                                crate::proto::to_object(rule.values.as_ref().unwrap())
                            {
                                map.insert("match".to_string(), Value::String(rule.r#type.clone()));
                                debug!("Creating matching rule with {:?}", map);
                                list.add_rule(
                                    &MatchingRule::from_json(&Value::Object(map)).unwrap(),
                                );
                            }
                            list
                        });
                (key.clone(), rules)
            })
            .collect();

        match (request.expected.as_ref(), request.actual.as_ref()) {
            (Some(expected), Some(actual)) => {
                let expected_sse = std::str::from_utf8(expected.content.as_ref().unwrap())
                    .map_err(|err| {
                        tonic::Status::aborted(format!("Failed to parse expected SSE: {}", err))
                    })?;
                let actual_sse =
                    std::str::from_utf8(actual.content.as_ref().unwrap()).map_err(|err| {
                        tonic::Status::aborted(format!("Failed to parse actual SSE: {}", err))
                    })?;
                compare_sse_contents(
                    expected_sse,
                    actual_sse,
                    request.allow_unexpected_keys,
                    &rules,
                )
                .map_err(|err| {
                    tonic::Status::aborted(format!("Failed to compare SSE contents: {}", err))
                })
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
                            diff: "".to_string(),
                            mismatch_type: "body".to_string(),
                          }
                        ]
                      }
                    },
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
                            diff: "".to_string(),
                            mismatch_type: "body".to_string(),
                          }
                        ]
                      }
                    },
                }))
            }
            (None, None) => Ok(Response::new(proto::CompareContentsResponse {
                error: String::default(),
                type_mismatch: None,
                results: hashmap! {},
            })),
        }
    }

    async fn configure_interaction(
        &self,
        request: tonic::Request<proto::ConfigureInteractionRequest>,
    ) -> Result<tonic::Response<proto::ConfigureInteractionResponse>, tonic::Status> {
        debug!(
            "Received configure_contents request for '{}'",
            request.get_ref().content_type
        );
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
                        content_type: contents
                            .content_type()
                            .unwrap_or(ContentType::from("text/event-stream"))
                            .to_string(),
                        content: Some(contents.value().unwrap().to_vec()),
                        content_type_hint: ContentTypeHint::Default as i32,
                    }),
                })
            })
            .map_err(|err| {
                tonic::Status::aborted(format!("Failed to generate SSE contents: {}", err))
            })
    }

    async fn start_mock_server(
        &self,
        _request: tonic::Request<proto::StartMockServerRequest>,
    ) -> Result<tonic::Response<proto::StartMockServerResponse>, tonic::Status> {
        Ok(Response::new(proto::StartMockServerResponse {
            response: Some(MockServerResponse::Error(
                "Mock server not implemented for SSE plugin".to_string(),
            )),
        }))
    }

    async fn shutdown_mock_server(
        &self,
        _request: tonic::Request<proto::MockServerRequest>,
    ) -> Result<tonic::Response<proto::MockServerResults>, tonic::Status> {
        Ok(Response::new(proto::MockServerResults {
            ok: true,
            results: vec![],
        }))
    }

    async fn get_mock_server_results(
        &self,
        _request: tonic::Request<proto::MockServerRequest>,
    ) -> Result<tonic::Response<proto::MockServerResults>, tonic::Status> {
        Ok(Response::new(proto::MockServerResults {
            ok: true,
            results: vec![],
        }))
    }

    async fn prepare_interaction_for_verification(
        &self,
        request: tonic::Request<proto::VerificationPreparationRequest>,
    ) -> Result<tonic::Response<proto::VerificationPreparationResponse>, tonic::Status> {
        let req = request.get_ref();
        debug!(
            "Prepare interaction for verification: interaction_type={}",
            req.interaction_contents
                .as_ref()
                .map(|ic| &ic.interaction_type)
                .unwrap_or(&String::new())
        );

        Ok(Response::new(proto::VerificationPreparationResponse {
            response: Some(PrepResponse::InteractionData(proto::InteractionData {
                body: Some(proto::Body {
                    content_type: "text/event-stream".to_string(),
                    content: None,
                    content_type_hint: ContentTypeHint::Default as i32,
                }),
                metadata: hashmap! {},
            })),
        }))
    }

    async fn verify_interaction(
        &self,
        request: tonic::Request<proto::VerifyInteractionRequest>,
    ) -> Result<tonic::Response<proto::VerifyInteractionResponse>, tonic::Status> {
        let req = request.get_ref();
        debug!("Verify interaction request");

        let interaction_data = req.interaction_data.as_ref();
        let interaction_contents = req.interaction_contents.as_ref();

        let plugin_config = interaction_contents
            .and_then(|ic| ic.plugin_configuration.as_ref());

        let rules = if let Some(config) = plugin_config {
            if let Some(ref interaction_config) = config.interaction_configuration {
                let mut rule_map = std::collections::HashMap::new();
                for (key, value) in &interaction_config.fields {
                    if key.ends_with("rules") {
                        if let Some(list_value) = &value.kind {
                            if let prost_types::value::Kind::ListValue(lv) = list_value {
                                for rule_val in &lv.values {
                                    if let Some(prost_types::value::Kind::StructValue(sv)) =
                                        &rule_val.kind
                                    {
                                        let rule_type = sv
                                            .fields
                                            .get("type")
                                            .and_then(|v| {
                                                if let Some(
                                                    prost_types::value::Kind::StringValue(s),
                                                ) = &v.kind
                                                {
                                                    Some(s.clone())
                                                } else {
                                                    None
                                                }
                                            })
                                            .unwrap_or_default();
                                        let mut map = serde_json::Map::new();
                                        for (k, v) in &sv.fields {
                                            map.insert(
                                                k.clone(),
                                                crate::utils::from_value(v),
                                            );
                                        }
                                        map.insert(
                                            "match".to_string(),
                                            Value::String(rule_type.clone()),
                                        );
                                        if let Ok(matching_rule) =
                                            MatchingRule::from_json(&Value::Object(map))
                                        {
                                            let path = key.trim_end_matches("rules");
                                            let entry = rule_map
                                                .entry(path.to_string())
                                                .or_insert_with(|| {
                                                    RuleList::empty(RuleLogic::And)
                                                });
                                            entry.add_rule(&matching_rule);
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                rule_map
            } else {
                std::collections::HashMap::new()
            }
        } else {
            std::collections::HashMap::new()
        };

        let expected_body = interaction_data
            .and_then(|id| id.body.as_ref())
            .and_then(|b| b.content.as_ref());

        let actual_body = req
            .interaction_data
            .as_ref()
            .and_then(|id| id.body.as_ref())
            .and_then(|b| b.content.as_ref());

        let mismatches = if let (Some(expected_bytes), Some(actual_bytes)) =
            (expected_body, actual_body)
        {
            let expected_sse = std::str::from_utf8(expected_bytes)
                .unwrap_or_else(|_| "error parsing expected SSE");
            let actual_sse = std::str::from_utf8(actual_bytes)
                .unwrap_or_else(|_| "error parsing actual SSE");

            match compare_sse_contents(expected_sse, actual_sse, true, &rules) {
                Ok(resp) => {
                    let mut items = Vec::new();
                    for (_, path_mismatches) in resp.into_inner().results {
                        for mm in path_mismatches.mismatches {
                            items.push(proto::VerificationResultItem {
                                result: Some(
                                    proto::verification_result_item::Result::Mismatch(mm),
                                ),
                            });
                        }
                    }
                    items
                }
                Err(e) => {
                    vec![proto::VerificationResultItem {
                        result: Some(proto::verification_result_item::Result::Error(
                            e.to_string(),
                        )),
                    }]
                }
            }
        } else if expected_body.is_some() && actual_body.is_none() {
            vec![proto::VerificationResultItem {
                result: Some(proto::verification_result_item::Result::Mismatch(
                    proto::ContentMismatch {
                        expected: expected_body.cloned(),
                        actual: None,
                        mismatch: "Expected SSE content, but did not get any".to_string(),
                        path: "".to_string(),
                        diff: "".to_string(),
                        mismatch_type: "body".to_string(),
                    },
                )),
            }]
        } else {
            vec![]
        };

        Ok(Response::new(proto::VerifyInteractionResponse {
            response: Some(VerifyResponse::Result(proto::VerificationResult {
                success: mismatches.is_empty(),
                response_data: None,
                mismatches,
                output: vec![],
            })),
        }))
    }
}

struct TcpIncoming {
    inner: TcpListener,
}

impl Stream for TcpIncoming {
    type Item = Result<TcpStream, std::io::Error>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Pin::new(&mut self.inner)
            .poll_accept(cx)
            .map_ok(|(stream, _)| stream)
            .map(Some)
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
    println!(
        "{{\"port\":{}, \"serverKey\":\"{}\"}}",
        address.port(),
        server_key
    );
    let _ = io::stdout().flush();

    let plugin = SsePactPlugin::default();
    Server::builder()
        .add_service(PactPluginServer::new(plugin))
        .serve_with_incoming(TcpIncoming { inner: listener })
        .await?;

    Ok(())
}
