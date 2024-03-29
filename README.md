# k8s-csi-restarter
Deletes pods that use given storage class on HTTP request 


| Env var                       | Required           | Example/Default           | Description  |
|-------------------------------|--------------------|---------------------------|--------------|
| RESTARTER_BEARER_TOKEN        | :white_check_mark: | password                  | Password that needs to be present in Authentication header |
| RESTARTER_STORAGE_CLASS       | :white_check_mark: | truenas-iscsi,truenas-nfs | List of storage class names, separated by comma |
| RESTARTER_BIND_ADDRESS        |                    | 0.0.0.0:8080              | Address to which http server will bind |
| RESTARTER_DRY_RUN             |                    | false                     | Run dry run instead of actually deleting pods |
| RESTARTER_DELETE_UNCONTROLLED |                    | false                     | Delete pods that don't have a controller (Deployment, DaemonSet, etc) as well |
| RUST_LOG                      |                    | warn                      | Configures logging level. To get any kind of input set to `info`, if you want some details without too much noise, set to `k8s_csi_restarter=debug` |
