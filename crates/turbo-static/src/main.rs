use std::{collections::HashMap, error::Error, fs, path::PathBuf};

use clap::Parser;
use fjall::{Config, Keyspace, PartitionCreateOptions};
use itertools::Itertools;
use lsp_server::{Connection, Message, Request, RequestId, Response};
use lsp_types::ClientCapabilities;

mod lsp_client;
mod visitor;

#[derive(Parser)]
struct Opt {
    #[clap(required = true)]
    paths: Vec<PathBuf>,

    /// reparse all files
    #[clap(long)]
    reparse: bool,

    /// reindex all files
    #[clap(long)]
    reindex: bool,
}

fn main() -> Result<(), Box<dyn Error>> {
    tracing_subscriber::fmt::init();
    let opt = Opt::parse();

    let mut connection = lsp_client::RAClient::new();
    connection.start(&opt.paths);

    // Each partition is its own physical LSM-tree
    let fjall = Config::new("file").open()?;

    tracing::info!("getting tasks");
    let tasks = get_all_tasks(&opt.paths);
    let dep_tree = resolve_tasks(&tasks, &mut connection, &fjall, opt.reindex);
    let concurrency = resolve_concurrency(&dep_tree);

    write_dep_tree(&dep_tree, std::path::Path::new("here"));

    Ok(())
}

/// search the given folders recursively and attempt to find all tasks inside
#[tracing::instrument(skip_all)]
fn get_all_tasks(folders: &[PathBuf]) -> Vec<(PathBuf, (syn::Ident, Vec<String>))> {
    let mut out = vec![];

    for folder in folders {
        let walker = ignore::Walk::new(folder);
        for entry in walker {
            let entry = entry.unwrap();
            let rs_file = if let Some(true) = entry.file_type().map(|t| t.is_file()) {
                let path = entry.path();
                let ext = path.extension().unwrap_or_default();
                if ext == "rs" {
                    std::fs::canonicalize(path).unwrap()
                } else {
                    continue;
                }
            } else {
                continue;
            };

            let file = fs::read_to_string(&rs_file).unwrap();
            let lines = file.lines();
            let mut occurences = vec![];

            tracing::debug!("processing {}", rs_file.display());

            for ((_, line), (line_no, fn_line)) in lines.enumerate().tuple_windows() {
                if line.contains("turbo_tasks::function") {
                    tracing::debug!("found at {:?}:L{}", rs_file, line_no);
                    let task_name = fn_line.to_owned();
                    occurences.push(line_no + 1);
                }
            }

            if occurences.is_empty() {
                continue;
            }

            // parse the file using syn and get the span of the functions
            let file = syn::parse_file(&file).unwrap();
            let occurences_count = occurences.len();
            let mut visitor = visitor::TaskVisitor::new();
            syn::visit::visit_file(&mut visitor, &file);
            if visitor.results.len() != occurences_count {
                tracing::warn!(
                    "file {:?} passed the heuristic with {:?} but the visitor found {:?}",
                    rs_file,
                    occurences_count,
                    visitor.results.len()
                );
            }

            out.extend(
                visitor
                    .results
                    .into_iter()
                    .map(move |ident| (rs_file.clone(), ident)),
            )
        }
    }

    out
}

fn resolve_tasks(
    tasks: &[(PathBuf, (syn::Ident, Vec<String>))],
    client: &mut lsp_client::RAClient,
    fjall: &fjall::Keyspace,
    reindex: bool,
) -> HashMap<String, Vec<(String, lsp_types::Range)>> {
    let items = fjall
        .open_partition("links", PartitionCreateOptions::default())
        .unwrap();

    let items = if reindex {
        fjall.delete_partition(items).unwrap();
        fjall
            .open_partition("links", PartitionCreateOptions::default())
            .unwrap()
    } else {
        items
    };

    tracing::info!(
        "found {} tasks, of which {} cached",
        tasks.len(),
        items.len().unwrap()
    );

    let mut out = HashMap::new();

    for (path, (ident, tags)) in tasks {
        let key = format!(
            "{}#{}:{}",
            path.display(),
            ident.to_string(),
            ident.span().start().line
        );
        if let Some(data) = items.get(&key).unwrap() {
            tracing::info!("skipping {}: got data {:?}", key, data);

            let data: Vec<(String, lsp_types::Range)> = bincode::deserialize(&data).unwrap();
            out.insert(key, data);
            continue;
        };

        tracing::info!("checking {} in {}", ident, path.display());

        let mut count = 0;
        let response = loop {
            let response = client.request(lsp_server::Request {
                id: 1.into(),
                method: "textDocument/prepareCallHierarchy".to_string(),
                params: serde_json::to_value(&lsp_types::CallHierarchyPrepareParams {
                    text_document_position_params: lsp_types::TextDocumentPositionParams {
                        position: lsp_types::Position {
                            line: ident.span().start().line as u32 - 1, // 0-indexed
                            character: ident.span().start().column as u32,
                        },
                        text_document: lsp_types::TextDocumentIdentifier {
                            uri: lsp_types::Url::from_file_path(&path).unwrap(),
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
                if value.len() != 0 {
                    break value.to_owned();
                }
                count += 1;
            }

            // textDocument/prepareCallHierarchy will sometimes return an empty array so try
            // at most 5 times
            if count > 5 {
                tracing::warn!("discovered isolated task {} in {}", ident, path.display());
                break vec![];
            }

            std::thread::sleep(std::time::Duration::from_secs(1));
        };

        // callHierarchy/incomingCalls
        let response = client.request(lsp_server::Request {
            id: 1.into(),
            method: "callHierarchy/incomingCalls".to_string(),
            params: serde_json::to_value(&lsp_types::CallHierarchyIncomingCallsParams {
                partial_result_params: lsp_types::PartialResultParams::default(),
                item: lsp_types::CallHierarchyItem {
                    name: ident.to_string().clone(),
                    kind: lsp_types::SymbolKind::FUNCTION,
                    data: None,
                    tags: None,
                    detail: None,
                    uri: lsp_types::Url::from_file_path(&path).unwrap(),
                    range: lsp_types::Range {
                        start: lsp_types::Position {
                            line: ident.span().start().line as u32 - 1, // 0-indexed
                            character: ident.span().start().column as u32,
                        },
                        end: lsp_types::Position {
                            line: ident.span().end().line as u32,
                            character: ident.span().end().column as u32,
                        },
                    },
                    selection_range: lsp_types::Range {
                        start: lsp_types::Position {
                            line: ident.span().start().line as u32 - 1, // 0-indexed
                            character: ident.span().start().column as u32,
                        },
                        end: lsp_types::Position {
                            line: ident.span().end().line as u32 - 1, // 0-indexed
                            character: ident.span().end().column as u32,
                        },
                    },
                },
                work_done_progress_params: lsp_types::WorkDoneProgressParams {
                    work_done_token: Some(lsp_types::ProgressToken::String("prepare".to_string())),
                },
            })
            .unwrap(),
        });

        let response: Result<Vec<lsp_types::CallHierarchyIncomingCall>, _> =
            serde_path_to_error::deserialize(response.result.unwrap());

        let links = response
            .unwrap()
            .into_iter()
            .map(|i| (i.from.uri.to_string(), i.from.selection_range))
            .collect::<Vec<(String, lsp_types::Range)>>();

        let data = bincode::serialize(&links).unwrap();

        tracing::info!("links: {:?}: {:?}", links, data);

        items.insert(key, data).unwrap();
        fjall.persist().unwrap();
        return out;
    }

    out
}

enum CallingStyle {
    Once,
    ZeroOrOnce,
    ZeroOrMore,
    OneOrMore,
}

/// given a map of tasks and functions that call it, produce a map of tasks and
/// those tasks that it calls

fn resolve_concurrency(
    dep_tree: &HashMap<String, Vec<(String, lsp_types::Range)>>,
) -> HashMap<String, Vec<(String, lsp_types::SelectionRange, CallingStyle)>> {
    Default::default()
}

fn write_dep_tree(
    dep_tree: &HashMap<String, Vec<(String, lsp_types::Range)>>,
    out: &std::path::Path,
) {
    let mut out = std::fs::File::create(out).unwrap();
    bincode::serialize_into(&mut out, dep_tree).unwrap();
}
