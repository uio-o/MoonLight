use crate::fx_rates::{fx_rate_table_json, read_fx_rate_table, refresh_fx_rates, FxRateTable};
use crate::handlers::admin::request::{AdminAppState, AdminRequestContext};
use crate::GatewayError;
use axum::{
    body::Body,
    http::{self, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;

fn table_response(table: &FxRateTable) -> Response<Body> {
    (
        [(http::header::CACHE_CONTROL, "private, no-store")],
        Json(json!({"item": fx_rate_table_json(table)})),
    )
        .into_response()
}

/// Read the stored rate table, or refresh it from the upstream feed.
///
/// Both live here rather than under the generic system-config endpoints so the
/// shape stays owned by the gateway: a read reports staleness alongside the
/// table, and a refresh is the only call in this area that leaves the process,
/// so its failure is reported as an upstream problem rather than a bad request.
pub(super) async fn response(
    state: &AdminAppState<'_>,
    context: &AdminRequestContext<'_>,
) -> Result<Option<Response<Body>>, GatewayError> {
    let path = context.path().trim_end_matches('/');
    if context.route_family() != Some("billing_manage")
        || (path != "/api/admin/billing/fx-rates"
            && path != "/api/admin/billing/fx-rates/refresh")
    {
        return Ok(None);
    }
    if context.method() == http::Method::GET && path == "/api/admin/billing/fx-rates" {
        let table = read_fx_rate_table(state.app()).await?;
        return Ok(Some(table_response(&table)));
    }
    if context.method() != http::Method::POST || path != "/api/admin/billing/fx-rates/refresh" {
        return Ok(None);
    }
    let table = match refresh_fx_rates(state.app()).await {
        Ok(table) => table,
        Err(err) => {
            let failure = (StatusCode::BAD_GATEWAY, Json(json!({"detail": err.into_message()})));
            return Ok(Some(failure.into_response()));
        }
    };
    Ok(Some(table_response(&table)))
}
