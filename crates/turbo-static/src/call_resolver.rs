use fjall::PartitionCreateOptions;

use crate::{lsp_client::RAClient, Identifier, IdentifierReference};

pub struct CallResolver<'a> {
    client: &'a mut RAClient,
    fjall: &'a fjall::Keyspace,
    handle: fjall::PartitionHandle,
}

impl<'a> CallResolver<'a> {
    pub fn new(client: &'a mut RAClient, fjall: &'a fjall::Keyspace) -> Self {
        let handle = fjall
            .open_partition("links", PartitionCreateOptions::default())
            .unwrap();
        Self {
            client,
            fjall,
            handle,
        }
    }

    pub fn cached(&self) -> usize {
        self.handle.len().unwrap()
    }

    pub fn cleared(mut self) -> Self {
        self.fjall.delete_partition(self.handle).unwrap();
        self.handle = self
            .fjall
            .open_partition("links", PartitionCreateOptions::default())
            .unwrap();
        self
    }

    pub fn resolve(&mut self, ident: &Identifier) -> Vec<IdentifierReference> {
        if let Some(data) = self.handle.get(ident.to_string()).unwrap() {
            tracing::info!("skipping {}", ident);
            return bincode::deserialize(&data).unwrap();
        };

        tracing::info!("checking {}", ident);

        let mut count = 0;
        let _response = loop {
            let response = self.client.request(lsp_server::Request {
                id: 1.into(),
                method: "textDocument/prepareCallHierarchy".to_string(),
                params: serde_json::to_value(&lsp_types::CallHierarchyPrepareParams {
                    text_document_position_params: lsp_types::TextDocumentPositionParams {
                        position: ident.range.start,
                        text_document: lsp_types::TextDocumentIdentifier {
                            uri: lsp_types::Url::from_file_path(&ident.path).unwrap(),
                        },
                    },
                    work_done_progress_params: lsp_types::WorkDoneProgressParams {
                        work_done_token: Some(lsp_types::ProgressToken::String(
                            "prepare".to_string(),
                        )),
                    },
                })
                .unwrap(),
            });
            if let Some(Some(value)) = response.result.as_ref().map(|r| r.as_array()) {
                if !value.is_empty() {
                    break value.to_owned();
                }
                count += 1;
            }

            // textDocument/prepareCallHierarchy will sometimes return an empty array so try
            // at most 5 times
            if count > 5 {
                tracing::warn!("discovered isolated task {}", ident);
                break vec![];
            }

            std::thread::sleep(std::time::Duration::from_secs(1));
        };

        // callHierarchy/incomingCalls
        let response = self.client.request(lsp_server::Request {
            id: 1.into(),
            method: "callHierarchy/incomingCalls".to_string(),
            params: serde_json::to_value(lsp_types::CallHierarchyIncomingCallsParams {
                partial_result_params: lsp_types::PartialResultParams::default(),
                item: lsp_types::CallHierarchyItem {
                    name: ident.name.to_owned(),
                    kind: lsp_types::SymbolKind::FUNCTION,
                    data: None,
                    tags: None,
                    detail: None,
                    uri: lsp_types::Url::from_file_path(&ident.path).unwrap(),
                    range: ident.range,
                    selection_range: ident.range,
                },
                work_done_progress_params: lsp_types::WorkDoneProgressParams {
                    work_done_token: Some(lsp_types::ProgressToken::String("prepare".to_string())),
                },
            })
            .unwrap(),
        });

        let links = if let Some(e) = response.error {
            tracing::warn!("unable to resolve {}: {:?}", ident, e);
            vec![]
        } else {
            let response: Result<Vec<lsp_types::CallHierarchyIncomingCall>, _> =
                serde_path_to_error::deserialize(response.result.unwrap());

            response
                .unwrap()
                .into_iter()
                .map(|i| i.into())
                .collect::<Vec<IdentifierReference>>()
        };

        let data = bincode::serialize(&links).unwrap();

        tracing::debug!("links: {:?}", links);

        self.handle.insert(ident.to_string(), data).unwrap();
        links
    }
}
