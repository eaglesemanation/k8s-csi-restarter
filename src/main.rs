use std::{collections::HashMap, net::SocketAddr};

use axum::{extract::State, http::StatusCode, routing::get, Router};
use config::Config;
use k8s_openapi::api::core::v1::{PersistentVolumeClaim as PVC, Pod};
use kube::api::{Api, DeleteParams, ListParams};
use serde::Deserialize;
use tower_http::{trace::TraceLayer, validate_request::ValidateRequestHeaderLayer};
use tracing::*;

// Configuration parsed on start from env or from TOML file
#[derive(Debug, Clone, Deserialize)]
struct Settings {
    pub bearer_token: String,
    pub storage_class: Vec<String>,
    #[serde(default = "default_bind_address")]
    pub bind_address: SocketAddr,
    #[serde(default)]
    pub delete_uncontrolled: bool,
    #[serde(default)]
    pub dry_run: bool,
}

fn default_bind_address() -> SocketAddr {
    "0.0.0.0:3000".parse().unwrap()
}

// State injected into route handlers
#[derive(Clone)]
struct AppState {
    pub k8s_client: kube::Client,
    pub settings: Settings,
}

// Error wrapper for auto conversion into response.
struct AppError(eyre::Report);

// Tell axum how to convert `AppError` into a response.
impl axum::response::IntoResponse for AppError {
    fn into_response(self) -> axum::http::Response<axum::body::Body> {
        error!("{:?}", self.0);
        (StatusCode::INTERNAL_SERVER_ERROR, "Something went wrong").into_response()
    }
}

// This enables using `?` on functions that return `eyre::Result<_>` to turn them into `Result<_, AppError>`.
impl<E> From<E> for AppError
where
    E: Into<eyre::Report>,
{
    fn from(err: E) -> Self {
        Self(err.into())
    }
}

#[tokio::main]
async fn main() -> eyre::Result<()> {
    color_eyre::install()?;
    tracing_subscriber::fmt::init();

    let settings_builder = Config::builder()
        .add_source(config::File::with_name("config").required(false))
        .add_source(
            config::Environment::with_prefix("RESTARTER")
                .try_parsing(true)
                .list_separator(","),
        )
        .build()?;
    let settings: Settings = settings_builder.try_deserialize()?;

    let k8s_client = kube::Client::try_default().await?;

    let app = Router::new()
        .route("/delete", get(delete_pods_with_pvc))
        .layer(ValidateRequestHeaderLayer::bearer(&settings.bearer_token))
        .layer(TraceLayer::new_for_http())
        .with_state(AppState {
            k8s_client,
            settings: settings.clone(),
        });

    let listner = tokio::net::TcpListener::bind(settings.bind_address).await?;
    info!("Listening on {}", settings.bind_address);
    Ok(axum::serve(listner, app).await?)
}

struct ObjectPath {
    pub namespace: String,
    pub name: String,
}

async fn get_pod_names_by_storage_class(
    k8s_client: kube::Client,
    storage_class: Vec<String>,
    skip_uncontrolled: bool,
) -> eyre::Result<Vec<ObjectPath>> {
    // Query for all PVCs and filter out those that use required storage class client side
    let pvcs_api: Api<PVC> = Api::all(k8s_client.clone());
    let pvcs = pvcs_api.list(&ListParams::default()).await?;
    let sc_pvc_paths: Vec<_> = pvcs
        .iter()
        .filter_map(|pvc| {
            let sc = pvc.spec.as_ref()?.storage_class_name.as_ref()?;
            if storage_class.contains(sc) {
                Some(format!(
                    "{}/{}",
                    pvc.metadata.namespace.as_ref()?,
                    pvc.metadata.name.as_ref()?
                ))
            } else {
                None
            }
        })
        .collect();
    info!(
        "Found {} PVCs that use one of these storage classes: {:?}",
        sc_pvc_paths.len(),
        storage_class
    );
    debug!("List of PVCs that use wanted storage class: {sc_pvc_paths:#?}");

    // Query for all pods and filter out those that mount one of previously found PVCs
    let pods_api: Api<Pod> = Api::all(k8s_client);
    let running_selector = &ListParams::default().fields("status.phase==Running");
    let pods = pods_api.list(running_selector).await?;
    let pvc_pods: Vec<_> = pods
        .iter()
        .filter_map(|pod| {
            let ns = pod.metadata.namespace.as_ref()?;
            let pod_name = pod.metadata.name.as_ref()?;
            // Exclude pods that do not have any controllers, otherwise it will not be recreated
            if pod.metadata.owner_references.as_ref()?.is_empty() && skip_uncontrolled {
                return None;
            }
            for vol in pod.spec.as_ref()?.volumes.as_ref()? {
                let Some(ref pvc) = vol.persistent_volume_claim else {
                    continue;
                };
                let pvc_path = format!("{}/{}", ns, pvc.claim_name);
                if sc_pvc_paths.contains(&pvc_path) {
                    return Some(ObjectPath {
                        namespace: ns.to_string(),
                        name: pod_name.to_string(),
                    });
                }
            }
            None
        })
        .collect();
    info!(
        "Found {} pods that use previously found PVCs",
        pvc_pods.len()
    );

    Ok(pvc_pods)
}

#[tracing::instrument(skip(state))]
async fn delete_pods_with_pvc(State(state): State<AppState>) -> Result<(), AppError> {
    info!(
        "Querying for pods that use PVCs with one of these storage classes: {:?}",
        state.settings.storage_class
    );
    let pvc_pods = get_pod_names_by_storage_class(
        state.k8s_client.clone(),
        state.settings.storage_class,
        !state.settings.delete_uncontrolled,
    )
    .await?;

    // Group pods by namespace
    let mut pvc_pods_by_namespace: HashMap<String, Vec<String>> = HashMap::new();
    for ObjectPath {
        namespace,
        ref name,
    } in pvc_pods
    {
        pvc_pods_by_namespace
            .entry(namespace)
            .and_modify(|pods| pods.push(name.to_string()))
            .or_insert(vec![name.to_string()]);
    }
    debug!("List of pods that use previously found PVCs: {pvc_pods_by_namespace:#?}");

    for (ns, pod_list) in pvc_pods_by_namespace {
        let pods_ns_api: Api<Pod> = Api::namespaced(state.k8s_client.clone(), &ns);
        let dp = DeleteParams {
            dry_run: state.settings.dry_run,
            ..Default::default()
        };
        for pod in pod_list {
            match pods_ns_api.delete(&pod, &dp).await? {
                either::Either::Left(_) => {
                    debug!("Deleting {ns}/{pod}");
                }
                either::Either::Right(status) => {
                    if status.is_failure() {
                        warn!("Failed to delete {ns}/{pod}");
                    } else {
                        debug!("Deleted {ns}/{pod}");
                    }
                }
            }
        }
    }

    info!("Pods deletion initiated successfully");
    Ok(())
}
