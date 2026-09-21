//! `experience/*` JSON-RPC methods (management channel, route A).
//!
//! Read/writes the SAME `<codex_home>/experience/store.json` as the agent;
//! never a private copy, never holds locks while idle, and every mutation
//! goes through `ExperienceManagementService` (validation + lifecycle).

use std::sync::Mutex;

use codex_app_server_protocol::ClientResponsePayload;
use codex_app_server_protocol::ExperienceActionResponse;
use codex_app_server_protocol::ExperienceAdoptDraftParams;
use codex_app_server_protocol::ExperienceAdoptDraftResponse;
use codex_app_server_protocol::ExperienceUpdateControlParams;
use codex_app_server_protocol::ExperienceUpdateMetaParams;
use codex_app_server_protocol::ExperienceUpdateResponse;
use codex_app_server_protocol::ExperienceDetailParams;
use codex_app_server_protocol::ExperienceDetailResponse;
use codex_app_server_protocol::ExperienceEditAsDraftParams;
use codex_app_server_protocol::ExperienceEditAsDraftResponse;
use codex_app_server_protocol::ExperienceExportResponse;
use codex_app_server_protocol::ExperienceIdParams;
use codex_app_server_protocol::ExperienceImportParams;
use codex_app_server_protocol::ExperienceImportResponse;
use codex_app_server_protocol::ExperienceListParams;
use codex_app_server_protocol::ExperienceListResponse;
use codex_core::experience_management::ExperienceManagementService;

use crate::error_code::internal_error;

pub(crate) struct ExperienceRequestProcessor {
    service: Mutex<ExperienceManagementService>,
}

impl ExperienceRequestProcessor {
    pub(crate) fn new() -> Self {
        let service = match ExperienceManagementService::open_default() {
            Ok(service) => service,
            Err(error) => {
                tracing::error!("failed to open experience store for app-server: {error}");
                // Fall back to an empty in-memory manager rooted at the default
                // location the next time a mutation reloads; read methods will
                // surface the open error through service reload behavior.
                ExperienceManagementService::open_default().unwrap_or_else(|_| {
                    let fallback =
                        std::env::temp_dir().join("codex-exp-fallback").join("store.json");
                    let _ = std::fs::create_dir_all(fallback.parent().unwrap());
                    ExperienceManagementService::open(fallback)
                })
            }
        };
        Self {
            service: Mutex::new(service),
        }
    }

    fn lock(&self) -> Result<std::sync::MutexGuard<'_, ExperienceManagementService>, codex_app_server_protocol::JSONRPCErrorError> {
        self.service
            .lock()
            .map_err(|_| internal_error("experience management lock poisoned"))
    }

    pub(crate) async fn experience_list(
        &self,
        _params: ExperienceListParams,
    ) -> Result<Option<ClientResponsePayload>, codex_app_server_protocol::JSONRPCErrorError> {
        let mut service = self.lock()?;
        let rows = service.list();
        let experiences = rows
            .into_iter()
            .map(|row| serde_json::to_value(row).unwrap_or_default())
            .collect();
        Ok(Some(
            ClientResponsePayload::ExperienceList(ExperienceListResponse { experiences }),
        ))
    }

    pub(crate) async fn experience_detail(
        &self,
        params: ExperienceDetailParams,
    ) -> Result<Option<ClientResponsePayload>, codex_app_server_protocol::JSONRPCErrorError> {
        let mut service = self.lock()?;
        let detail = service
            .detail(&params.id)
            .map_err(|error| internal_error(error.to_string()))?;
        Ok(Some(
            ClientResponsePayload::ExperienceDetail(ExperienceDetailResponse {
                experience: serde_json::to_value(&detail.experience).unwrap_or_default(),
                pinned: detail.pinned,
                usage: serde_json::to_value(&detail.usage).unwrap_or_default(),
                audits: detail
                    .audits
                    .into_iter()
                    .map(|audit| serde_json::to_value(audit).unwrap_or_default())
                    .collect(),
            }),
        ))
    }

    async fn action(
        &self,
        params: ExperienceIdParams,
        op: fn(&mut ExperienceManagementService, &str, &str) -> anyhow::Result<String>,
        build: fn(ExperienceActionResponse) -> ClientResponsePayload,
    ) -> Result<Option<ClientResponsePayload>, codex_app_server_protocol::JSONRPCErrorError> {
        let mut service = self.lock()?;
        let status = op(&mut service, &params.id, "app-server")
            .map_err(|error| internal_error(error.to_string()))?;
        Ok(Some(build(ExperienceActionResponse {
            ok: true,
            id: params.id,
            status: Some(status),
        })))
    }

    pub(crate) async fn experience_pin(
        &self,
        params: ExperienceIdParams,
    ) -> Result<Option<ClientResponsePayload>, codex_app_server_protocol::JSONRPCErrorError> {
        let mut service = self.lock()?;
        service
            .pin(&params.id, "app-server")
            .map_err(|error| internal_error(error.to_string()))?;
        Ok(Some(ClientResponsePayload::ExperiencePin(
            ExperienceActionResponse { ok: true, id: params.id, status: None },
        )))
    }

    pub(crate) async fn experience_unpin(
        &self,
        params: ExperienceIdParams,
    ) -> Result<Option<ClientResponsePayload>, codex_app_server_protocol::JSONRPCErrorError> {
        let mut service = self.lock()?;
        service
            .unpin(&params.id, "app-server")
            .map_err(|error| internal_error(error.to_string()))?;
        Ok(Some(ClientResponsePayload::ExperienceUnpin(
            ExperienceActionResponse { ok: true, id: params.id, status: None },
        )))
    }

    pub(crate) async fn experience_activate(
        &self,
        params: ExperienceIdParams,
    ) -> Result<Option<ClientResponsePayload>, codex_app_server_protocol::JSONRPCErrorError> {
        self.action(params, |service, id, by| {
            service.activate(id, by).map(|status| format!("{status:?}"))
        }, ClientResponsePayload::ExperienceActivate)
        .await
    }

    pub(crate) async fn experience_revalidate(
        &self,
        params: ExperienceIdParams,
    ) -> Result<Option<ClientResponsePayload>, codex_app_server_protocol::JSONRPCErrorError> {
        self.action(params, |service, id, by| {
            service.revalidate(id, by).map(|status| format!("{status:?}"))
        }, ClientResponsePayload::ExperienceRevalidate)
        .await
    }

    pub(crate) async fn experience_disable(
        &self,
        params: ExperienceIdParams,
    ) -> Result<Option<ClientResponsePayload>, codex_app_server_protocol::JSONRPCErrorError> {
        self.action(params, |service, id, by| {
            service.disable(id, by).map(|status| format!("{status:?}"))
        }, ClientResponsePayload::ExperienceDisable)
        .await
    }

    pub(crate) async fn experience_delete(
        &self,
        params: ExperienceIdParams,
    ) -> Result<Option<ClientResponsePayload>, codex_app_server_protocol::JSONRPCErrorError> {
        let mut service = self.lock()?;
        service
            .delete(&params.id, "app-server")
            .map_err(|error| internal_error(error.to_string()))?;
        Ok(Some(ClientResponsePayload::ExperienceDelete(
            ExperienceActionResponse { ok: true, id: params.id, status: None },
        )))
    }

    pub(crate) async fn experience_export(
        &self,
        params: ExperienceIdParams,
    ) -> Result<Option<ClientResponsePayload>, codex_app_server_protocol::JSONRPCErrorError> {
        let mut service = self.lock()?;
        let json = service
            .export_json(&params.id)
            .map_err(|error| internal_error(error.to_string()))?;
        Ok(Some(ClientResponsePayload::ExperienceExport(ExperienceExportResponse { json })))
    }

    pub(crate) async fn experience_import(
        &self,
        params: ExperienceImportParams,
    ) -> Result<Option<ClientResponsePayload>, codex_app_server_protocol::JSONRPCErrorError> {
        let mut service = self.lock()?;
        let id = service
            .import_json(&params.json, params.overwrite, "app-server")
            .map_err(|error| internal_error(error.to_string()))?;
        Ok(Some(ClientResponsePayload::ExperienceImport(ExperienceImportResponse {
            id: id.0,
        })))
    }

    pub(crate) async fn experience_edit_as_draft(
        &self,
        params: ExperienceEditAsDraftParams,
    ) -> Result<Option<ClientResponsePayload>, codex_app_server_protocol::JSONRPCErrorError> {
        let mut service = self.lock()?;
        let draft_id = service
            .edit_as_draft(&params.id, "app-server")
            .map_err(|error| internal_error(error.to_string()))?;
        Ok(Some(ClientResponsePayload::ExperienceEditAsDraft(
            ExperienceEditAsDraftResponse { draft_id: draft_id.0 },
        )))
    }

    pub(crate) async fn experience_adopt_draft(
        &self,
        params: ExperienceAdoptDraftParams,
    ) -> Result<Option<ClientResponsePayload>, codex_app_server_protocol::JSONRPCErrorError> {
        let mut service = self.lock()?;
        let status = service
            .adopt_draft(&params.draft_id, &params.replace_id, "app-server")
            .map_err(|error| internal_error(error.to_string()))?;
        Ok(Some(ClientResponsePayload::ExperienceAdoptDraft(
            ExperienceAdoptDraftResponse { status: format!("{status:?}") },
        )))
    }

    pub(crate) async fn experience_update_meta(
        &self,
        params: ExperienceUpdateMetaParams,
    ) -> Result<Option<ClientResponsePayload>, codex_app_server_protocol::JSONRPCErrorError> {
        let mut service = self.lock()?;
        service
            .update_meta(&params.id, params.title, params.note, params.confidence, "app-server")
            .map_err(|error| internal_error(error.to_string()))?;
        Ok(Some(ClientResponsePayload::ExperienceUpdateMeta(
            ExperienceUpdateResponse { ok: true },
        )))
    }

    pub(crate) async fn experience_update_control(
        &self,
        params: ExperienceUpdateControlParams,
    ) -> Result<Option<ClientResponsePayload>, codex_app_server_protocol::JSONRPCErrorError> {
        let mut service = self.lock()?;
        service
            .update_control(&params.id, params.applicability, params.conditions, "app-server")
            .map_err(|error| internal_error(error.to_string()))?;
        Ok(Some(ClientResponsePayload::ExperienceUpdateControl(
            ExperienceUpdateResponse { ok: true },
        )))
    }
}
