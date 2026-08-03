//! Which control units a car has, and what each one says it is.
//!
//! Every scaling this tool applies is looked up by what a unit reported about
//! itself — its part number, and the ODX file it names — so the walk that
//! collects those answers is the first thing any live command does. It was
//! written inside `watch`, where a second live command cannot reach it; both
//! `race` and its setup need the same walk, and a copy of it would drift from
//! this one the first time either learned something.
//!
//! Nothing here changes a unit: three identifiers are read and no session is
//! opened. `SAFETY.md` is about what a sweep can provoke, and this is not one.

use std::time::Duration;

use vag_protocol::AsyncUdsClient;
use vag_protocol::address::UnitAddress;
use vag_transport::CanId;

use crate::watch::plan::{self, UnitIdentity};

/// Ask the car which control units it has, then ask each of them what it is.
///
/// `also` is the units the caller wants identified whatever the gateway says —
/// what it means to poll, in practice. The adapter comes back out because it is
/// a single-user resource with no way to borrow it across an await, so it is
/// handed over and handed back rather than shared.
pub async fn identify<B: vag_can::CanBackend>(
    backend: B,
    also: &[u16],
    progress: &mut crate::progress::Line,
) -> (B, Vec<UnitIdentity>) {
    let mut wanted: Vec<u16> = also.to_vec();
    // Which units the car has. Without this the view would only ever show the
    // engine, because a unit with no identity contributes no channels and so
    // no tab — which is what "switching between units does nothing" looked
    // like. One read of the gateway's installation list answers it, the same
    // read `vagcan units` makes; a car whose gateway does not answer falls
    // back to whatever was asked for.
    progress.update("asking the gateway which control units this car has");
    let gateway =
        UnitAddress::from_request(0x710).expect("the gateway is in VW's block");
    let mut uds = AsyncUdsClient::new(vag_can::IsoTpCan::new(
        backend,
        CanId::Standard(gateway.request),
        CanId::Standard(gateway.response),
    ));
    if let Ok(bitmap) =
        uds.read_data_by_identifier(vag_protocol::gateway::INSTALLATION_LIST).await
    {
        wanted.extend(vag_protocol::gateway::decode_installation_list(&bitmap));
    }
    // The powertrain is never in that list — it lives on the other id
    // block — so it is added rather than discovered.
    wanted.push(0x7E1);
    let backend = uds.into_transport().into_backend();

    identify_listed(backend, &wanted, &[], progress).await
}

/// Identify a named list of units, skipping the ones already accounted for.
///
/// Split out from [`identify`] for the caller that has another source for the
/// list: a survey recording already asked every unit for its identification
/// block, so re-reading those costs a probe apiece for an answer in hand — but
/// whatever the survey did *not* cover still has to be asked. Only the units
/// newly identified come back, so the caller keeps the order it had.
pub async fn identify_listed<B: vag_can::CanBackend>(
    mut backend: B,
    requests: &[u16],
    known: &[UnitIdentity],
    progress: &mut crate::progress::Line,
) -> (B, Vec<UnitIdentity>) {
    let mut wanted = requests.to_vec();
    wanted.sort_unstable();
    wanted.dedup();
    let mut identities: Vec<UnitIdentity> = Vec::new();
    let total = wanted.len();
    for (at, request) in wanted.into_iter().enumerate() {
        progress
            .update(&format!("identifying control units — {request:03X}, {} of {total}", at + 1));
        if known.iter().chain(identities.iter()).any(|i| i.request == request) {
            continue;
        }
        let Some(address) = UnitAddress::from_request(request) else {
            continue;
        };
        let mut uds = AsyncUdsClient::new(vag_can::IsoTpCan::new(
            backend,
            CanId::Standard(address.request),
            CanId::Standard(address.response),
        ));
        let text = |data: Option<Vec<u8>>| {
            data.map(|b| String::from_utf8_lossy(&b).trim_end_matches(['\0', ' ']).to_string())
                .filter(|s| !s.is_empty())
        };
        // One short probe decides whether the unit is there. A unit that is
        // not costs this deadline once, instead of the full two-second one
        // three times over — fifteen listed addresses at that price is what
        // made startup take several seconds.
        const PROBE: Duration = Duration::from_millis(300);
        let part = text(uds.read_data_by_identifier_within(0xF187, PROBE).await.ok());
        if part.is_none() && request != plan::ENGINE {
            backend = uds.into_transport().into_backend();
            continue;
        }
        // Identification only — no session change and no sweep. `SAFETY.md`
        // is about what a sweep can provoke; this is not one.
        let component = text(uds.read_data_by_identifier_within(0xF197, PROBE).await.ok());
        let odx = text(uds.read_data_by_identifier_within(0xF19E, PROBE).await.ok());
        identities.push(UnitIdentity {
            request,
            part_number: part,
            odx_name: odx,
            component,
        });
        backend = uds.into_transport().into_backend();
    }
    (backend, identities)
}
