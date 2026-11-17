//! Platform-neutral meaning of the configured audio route.
//!
//! This module describes intent and observations only. It does not open a
//! device, start a process, or claim that a saved route is currently active.

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", content = "deviceId", rename_all = "snake_case")]
pub enum AudioDestination {
    SystemDefault,
    Device(String),
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AudioAccess {
    Shared,
    Exclusive,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AudioProcessing {
    Off,
    Equalizer,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AudioRouteState {
    Requested,
    Transitioning,
    Confirmed,
    Uncertain,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AudioRoute {
    pub destination: AudioDestination,
    pub access: AudioAccess,
    pub processing: AudioProcessing,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LegacyRouteIssue {
    LegacyAlsaDefaultMapsToShared,
    ExplicitDeviceIgnoredInSharedMode,
    UnknownOutputTypeUsesLegacyMapping,
    InvalidDeviceIdentifier,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LegacyRouteInterpretation {
    pub route: AudioRoute,
    pub issues: Vec<LegacyRouteIssue>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AudioRouteSnapshot {
    pub state: AudioRouteState,
    pub requested: AudioRoute,
    /// Present only when this route has a confirmed observation. A saved
    /// configuration alone must never populate it.
    pub confirmed: Option<AudioRoute>,
}

/// Lifecycle of the latest route-related operation. This is an observation,
/// not a command to start or stop an audio route.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RouteOperationState {
    Stable,
    Transitioning,
    Failed,
    Uncertain,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RouteSupervisorState {
    Starting,
    Available,
    Unavailable,
    Stopping,
    Failed,
    Unknown,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RouteDspState {
    Unavailable,
    Off,
    Preparing,
    Ready,
    ActiveVerified,
    ActiveUnverified,
    Updating,
    Stopping,
    Failed,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RoutePlatform {
    Linux,
    Windows,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RouteEvidence {
    PersistedConfiguration,
    SupervisorOutputApplied,
    MpdProtocolAvailable,
    PlaybackSnapshotConsistent,
    DirectPcmObserved,
    DspGraphVerified,
    WindowsSessionObserved,
}

/// Sanitized reason codes only; raw MPD, device and filesystem errors are not
/// copied into the route status payload.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RouteDiagnostic {
    ConfigurationUnavailable,
    InvalidConfiguration,
    AmbiguousLegacyConfiguration,
    SupervisorUnavailable,
    SupervisorFailed,
    ConfigurationFailed,
    ConfigurationUncertain,
    OutputNotApplied,
    OutputConfigurationChanged,
    PlaybackSnapshotInconsistent,
    DirectPcmNotObserved,
    DspNotVerified,
    DspFailed,
    DspRequiresSharedAccess,
    NullOutputTestMode,
    WindowsExclusiveNotObservable,
}

/// A platform-neutral projection composed from read-only observations.
/// `route` can be absent only when the persisted request is missing or invalid.
/// `applied` means the current owned software session accepted that output
/// configuration; it does not imply that PCM is acquired or audio is audible.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AudioRouteProjection {
    pub generation: u64,
    pub operation_id: Option<String>,
    pub operation: RouteOperationState,
    pub route: Option<AudioRouteSnapshot>,
    pub applied: Option<AudioRoute>,
    pub evidence: Vec<RouteEvidence>,
    pub diagnostic: Option<RouteDiagnostic>,
    /// `Some(true)` is emitted only for a positively observed Linux Direct PCM.
    /// Shared and Windows paths remain unknown here.
    pub pcm_acquired: Option<bool>,
    /// Windows currently has no observation that proves WASAPI exclusivity.
    pub exclusive_access_confirmed: Option<bool>,
    pub legacy_issues: Vec<LegacyRouteIssue>,
}

pub struct RouteProjectionInput {
    pub persisted: Option<LegacyRouteInterpretation>,
    pub applied: Option<AudioRoute>,
    pub supervisor: RouteSupervisorState,
    pub playback_consistent: bool,
    pub playing: bool,
    /// True only when the platform adapter matched the observation to the
    /// selected endpoint and owned MPD process.
    pub direct_pcm_observed: bool,
    pub dsp: RouteDspState,
    pub operation: RouteOperationState,
    pub diagnostic: Option<RouteDiagnostic>,
    pub operation_id: Option<String>,
    pub generation: u64,
    pub platform: RoutePlatform,
    pub null_output_test: bool,
}

/// Builds the common route status without performing I/O or interpreting
/// platform-specific device names. Adapters provide only already-collected
/// evidence and the current persisted/applied route descriptions.
pub fn project_audio_route(input: RouteProjectionInput) -> AudioRouteProjection {
    let Some(persisted) = input.persisted else {
        return empty_projection(
            input.generation,
            input.operation_id,
            RouteOperationState::Uncertain,
            RouteDiagnostic::ConfigurationUnavailable,
        );
    };
    let mut evidence = vec![RouteEvidence::PersistedConfiguration];
    let mut requested = persisted.route.clone();
    let wants_eq = matches!(
        input.dsp,
        RouteDspState::Preparing
            | RouteDspState::Ready
            | RouteDspState::ActiveVerified
            | RouteDspState::ActiveUnverified
            | RouteDspState::Updating
            | RouteDspState::Failed
    );
    let mut diagnostic = input.diagnostic;
    if wants_eq && requested.access != AudioAccess::Shared {
        diagnostic.get_or_insert(RouteDiagnostic::DspRequiresSharedAccess);
    } else if wants_eq {
        requested.processing = AudioProcessing::Equalizer;
    }
    if requested.validate().is_err()
        || persisted
            .issues
            .contains(&LegacyRouteIssue::InvalidDeviceIdentifier)
    {
        return AudioRouteProjection {
            generation: input.generation,
            operation_id: input.operation_id,
            operation: RouteOperationState::Uncertain,
            route: None,
            applied: None,
            evidence,
            diagnostic: Some(RouteDiagnostic::InvalidConfiguration),
            pcm_acquired: None,
            exclusive_access_confirmed: None,
            legacy_issues: persisted.issues,
        };
    }
    if persisted
        .issues
        .contains(&LegacyRouteIssue::UnknownOutputTypeUsesLegacyMapping)
    {
        diagnostic.get_or_insert(RouteDiagnostic::AmbiguousLegacyConfiguration);
    }

    let mut applied = input.applied.filter(|route| route.validate().is_ok());
    if let Some(route) = &applied {
        evidence.push(RouteEvidence::SupervisorOutputApplied);
        if route.processing != AudioProcessing::Off {
            let mut base = route.clone();
            base.processing = AudioProcessing::Off;
            applied = Some(base);
        }
    }
    if input.supervisor == RouteSupervisorState::Available {
        evidence.push(RouteEvidence::MpdProtocolAvailable);
    }
    if input.playback_consistent {
        evidence.push(RouteEvidence::PlaybackSnapshotConsistent);
    }
    if input.platform == RoutePlatform::Windows
        && input.supervisor == RouteSupervisorState::Available
    {
        evidence.push(RouteEvidence::WindowsSessionObserved);
    }

    let requested_base = without_processing(&requested);
    let applied_matches_request = applied
        .as_ref()
        .is_some_and(|route| same_output_route(route, &requested_base));
    let dsp_transitioning = matches!(
        input.dsp,
        RouteDspState::Preparing
            | RouteDspState::Ready
            | RouteDspState::Updating
            | RouteDspState::Stopping
    );
    let config_transitioning = input.operation == RouteOperationState::Transitioning;
    let output_transitioning = input.supervisor == RouteSupervisorState::Starting
        || input.supervisor == RouteSupervisorState::Stopping;

    let mut operation = if matches!(input.supervisor, RouteSupervisorState::Failed)
        || input.dsp == RouteDspState::Failed
        || input.operation == RouteOperationState::Failed
    {
        diagnostic.get_or_insert(if input.dsp == RouteDspState::Failed {
            RouteDiagnostic::DspFailed
        } else {
            RouteDiagnostic::SupervisorFailed
        });
        RouteOperationState::Failed
    } else if matches!(
        diagnostic,
        Some(
            RouteDiagnostic::DspRequiresSharedAccess
                | RouteDiagnostic::AmbiguousLegacyConfiguration
        )
    ) {
        RouteOperationState::Uncertain
    } else if input.operation == RouteOperationState::Uncertain {
        RouteOperationState::Uncertain
    } else if output_transitioning || dsp_transitioning || config_transitioning {
        RouteOperationState::Transitioning
    } else if matches!(
        input.supervisor,
        RouteSupervisorState::Unavailable | RouteSupervisorState::Unknown
    ) || !input.playback_consistent
        || input.dsp == RouteDspState::ActiveUnverified
    {
        RouteOperationState::Uncertain
    } else if !applied_matches_request {
        RouteOperationState::Uncertain
    } else {
        RouteOperationState::Stable
    };

    if input.null_output_test {
        operation = RouteOperationState::Uncertain;
        diagnostic = Some(RouteDiagnostic::NullOutputTestMode);
        applied = None;
    } else if operation == RouteOperationState::Failed {
        diagnostic.get_or_insert(if input.dsp == RouteDspState::Failed {
            RouteDiagnostic::DspFailed
        } else {
            RouteDiagnostic::SupervisorFailed
        });
    } else if operation == RouteOperationState::Transitioning {
        // The reason is represented by operation/dsp state; do not expose raw errors.
    } else if input.supervisor != RouteSupervisorState::Available {
        diagnostic.get_or_insert(RouteDiagnostic::SupervisorUnavailable);
    } else if !input.playback_consistent {
        diagnostic.get_or_insert(RouteDiagnostic::PlaybackSnapshotInconsistent);
    } else if !applied_matches_request && !config_transitioning {
        diagnostic.get_or_insert(RouteDiagnostic::OutputConfigurationChanged);
    } else if applied.is_none() {
        diagnostic.get_or_insert(RouteDiagnostic::OutputNotApplied);
    }

    let mut pcm_acquired = None;
    let mut exclusive_access_confirmed = None;
    let confirmed = if operation != RouteOperationState::Stable {
        None
    } else if requested.processing == AudioProcessing::Equalizer {
        if input.dsp == RouteDspState::ActiveVerified {
            evidence.push(RouteEvidence::DspGraphVerified);
            Some(requested.clone())
        } else {
            diagnostic.get_or_insert(RouteDiagnostic::DspNotVerified);
            None
        }
    } else if requested.access == AudioAccess::Shared {
        applied_matches_request.then(|| requested.clone())
    } else if input.platform == RoutePlatform::Windows {
        diagnostic.get_or_insert(RouteDiagnostic::WindowsExclusiveNotObservable);
        None
    } else if input.playing && input.direct_pcm_observed {
        evidence.push(RouteEvidence::DirectPcmObserved);
        pcm_acquired = Some(true);
        exclusive_access_confirmed = Some(true);
        Some(requested.clone())
    } else {
        diagnostic.get_or_insert(RouteDiagnostic::DirectPcmNotObserved);
        None
    };

    let may_retain_previous_route = !matches!(
        input.dsp,
        RouteDspState::Updating
            | RouteDspState::Stopping
            | RouteDspState::ActiveUnverified
            | RouteDspState::Failed
    );
    let previous_confirmed = if confirmed.is_none()
        && operation == RouteOperationState::Transitioning
        && may_retain_previous_route
    {
        applied.as_ref().and_then(|route| {
            let mut prior = route.clone();
            prior.processing = AudioProcessing::Off;
            let route_matches_selected_output =
                same_output_route(&prior, &requested_base) || config_transitioning;
            let direct_is_observed = prior.access != AudioAccess::Exclusive
                || (input.platform == RoutePlatform::Linux
                    && input.playing
                    && input.direct_pcm_observed);
            (route_matches_selected_output && direct_is_observed && input.playback_consistent)
                .then_some(prior)
        })
    } else {
        None
    };

    let snapshot = match operation {
        RouteOperationState::Transitioning => {
            AudioRouteSnapshot::transitioning(requested, previous_confirmed)
        }
        RouteOperationState::Failed | RouteOperationState::Uncertain => {
            AudioRouteSnapshot::uncertain(requested)
        }
        RouteOperationState::Stable => match confirmed {
            Some(route) => AudioRouteSnapshot::confirmed(requested, route),
            None => AudioRouteSnapshot::requested(requested),
        },
    }
    .ok();

    if operation == RouteOperationState::Uncertain && diagnostic.is_none() {
        diagnostic = Some(if input.dsp == RouteDspState::ActiveUnverified {
            RouteDiagnostic::DspNotVerified
        } else {
            RouteDiagnostic::OutputNotApplied
        });
    }

    AudioRouteProjection {
        generation: input.generation,
        operation_id: input.operation_id,
        operation,
        route: snapshot,
        applied,
        evidence,
        diagnostic,
        pcm_acquired,
        exclusive_access_confirmed,
        legacy_issues: persisted.issues,
    }
}

fn empty_projection(
    generation: u64,
    operation_id: Option<String>,
    operation: RouteOperationState,
    diagnostic: RouteDiagnostic,
) -> AudioRouteProjection {
    AudioRouteProjection {
        generation,
        operation_id,
        operation,
        route: None,
        applied: None,
        evidence: Vec::new(),
        diagnostic: Some(diagnostic),
        pcm_acquired: None,
        exclusive_access_confirmed: None,
        legacy_issues: Vec::new(),
    }
}

fn without_processing(route: &AudioRoute) -> AudioRoute {
    let mut route = route.clone();
    route.processing = AudioProcessing::Off;
    route
}

fn same_output_route(a: &AudioRoute, b: &AudioRoute) -> bool {
    a.destination == b.destination && a.access == b.access
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RouteContractError {
    InvalidDeviceIdentifier,
    ExclusiveSystemDefault,
    EqualizerRequiresSharedAccess,
    InvalidSnapshotState,
    RequestedAndObservedRouteDiffer,
}

impl AudioRoute {
    pub fn validate(&self) -> Result<(), RouteContractError> {
        match &self.destination {
            AudioDestination::SystemDefault if self.access == AudioAccess::Exclusive => {
                return Err(RouteContractError::ExclusiveSystemDefault);
            }
            AudioDestination::Device(id)
                if id.is_empty() || id == "default" || id.chars().any(char::is_control) =>
            {
                return Err(RouteContractError::InvalidDeviceIdentifier);
            }
            _ => {}
        }
        if self.processing == AudioProcessing::Equalizer && self.access != AudioAccess::Shared {
            return Err(RouteContractError::EqualizerRequiresSharedAccess);
        }
        Ok(())
    }
}

impl AudioRouteSnapshot {
    pub fn requested(route: AudioRoute) -> Result<Self, RouteContractError> {
        route.validate()?;
        Ok(Self {
            state: AudioRouteState::Requested,
            requested: route,
            confirmed: None,
        })
    }

    pub fn transitioning(
        requested: AudioRoute,
        previously_confirmed: Option<AudioRoute>,
    ) -> Result<Self, RouteContractError> {
        requested.validate()?;
        if let Some(route) = &previously_confirmed {
            route.validate()?;
        }
        Ok(Self {
            state: AudioRouteState::Transitioning,
            requested,
            confirmed: previously_confirmed,
        })
    }

    pub fn confirmed(
        requested: AudioRoute,
        observed: AudioRoute,
    ) -> Result<Self, RouteContractError> {
        requested.validate()?;
        observed.validate()?;
        if requested != observed {
            return Err(RouteContractError::RequestedAndObservedRouteDiffer);
        }
        Ok(Self {
            state: AudioRouteState::Confirmed,
            requested,
            confirmed: Some(observed),
        })
    }

    pub fn uncertain(requested: AudioRoute) -> Result<Self, RouteContractError> {
        requested.validate()?;
        Ok(Self {
            state: AudioRouteState::Uncertain,
            requested,
            confirmed: None,
        })
    }

    pub fn validate(&self) -> Result<(), RouteContractError> {
        self.requested.validate()?;
        if let Some(route) = &self.confirmed {
            route.validate()?;
        }
        match self.state {
            AudioRouteState::Requested | AudioRouteState::Uncertain if self.confirmed.is_some() => {
                Err(RouteContractError::InvalidSnapshotState)
            }
            AudioRouteState::Confirmed if self.confirmed.as_ref() != Some(&self.requested) => {
                Err(RouteContractError::InvalidSnapshotState)
            }
            _ => Ok(()),
        }
    }
}

/// Interprets existing persisted fields without migrating or rewriting them.
/// This deliberately matches the currently deployed Linux/Windows-compatible
/// mapping, while retaining diagnostics for legacy or contradictory values.
pub fn interpret_legacy_route(output_type: &str, device: &str) -> LegacyRouteInterpretation {
    let known_type = matches!(output_type, "alsa" | "pipewire" | "shared");
    let shared_mode = matches!(output_type, "pipewire" | "shared");
    let uses_shared_access = shared_mode || device == "default";
    let mut issues = Vec::new();

    if !known_type {
        issues.push(LegacyRouteIssue::UnknownOutputTypeUsesLegacyMapping);
    }
    if output_type == "alsa" && device == "default" {
        issues.push(LegacyRouteIssue::LegacyAlsaDefaultMapsToShared);
    }
    if shared_mode && device != "default" {
        issues.push(LegacyRouteIssue::ExplicitDeviceIgnoredInSharedMode);
    }
    if !uses_shared_access && (device.is_empty() || device.chars().any(char::is_control)) {
        issues.push(LegacyRouteIssue::InvalidDeviceIdentifier);
    }

    LegacyRouteInterpretation {
        route: AudioRoute {
            destination: if uses_shared_access {
                AudioDestination::SystemDefault
            } else {
                AudioDestination::Device(device.to_owned())
            },
            access: if uses_shared_access {
                AudioAccess::Shared
            } else {
                AudioAccess::Exclusive
            },
            // The legacy route fields do not prove or persist live DSP state.
            processing: AudioProcessing::Off,
        },
        issues,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;
    use serde_json::Value;

    #[derive(Deserialize)]
    struct Fixture {
        cases: Vec<FixtureCase>,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct FixtureCase {
        name: String,
        output_type: String,
        device: String,
        expected: Value,
    }

    #[test]
    fn legacy_interpretation_matches_shared_frontend_fixture() {
        let fixture: Fixture = serde_json::from_str(include_str!(
            "../../tests/fixtures/audio-route-contract.json"
        ))
        .expect("shared audio route fixture is valid JSON");
        for case in fixture.cases {
            let interpretation = interpret_legacy_route(&case.output_type, &case.device);
            let actual = serde_json::to_value(&interpretation).expect("interpretation serializes");
            assert_eq!(actual, case.expected, "fixture case: {}", case.name);
            assert_eq!(
                interpretation.route.validate().is_ok(),
                !interpretation
                    .issues
                    .contains(&LegacyRouteIssue::InvalidDeviceIdentifier),
                "fixture validity: {}",
                case.name
            );
        }
    }

    fn route(access: AudioAccess, processing: AudioProcessing, device: &str) -> AudioRoute {
        AudioRoute {
            destination: AudioDestination::Device(device.into()),
            access,
            processing,
        }
    }

    #[test]
    fn saved_routes_are_requested_and_eq_is_never_inferred_from_config() {
        let configured = interpret_legacy_route("pipewire", "default").route;
        let snapshot = AudioRouteSnapshot::requested(configured.clone()).unwrap();
        assert_eq!(snapshot.state, AudioRouteState::Requested);
        assert_eq!(snapshot.requested.processing, AudioProcessing::Off);
        assert!(snapshot.confirmed.is_none());

        let eq = route(
            AudioAccess::Shared,
            AudioProcessing::Equalizer,
            "pipewire:sink-1",
        );
        let requested = AudioRouteSnapshot::requested(eq.clone()).unwrap();
        assert_eq!(requested.requested.processing, AudioProcessing::Equalizer);
        assert!(requested.confirmed.is_none());
        let confirmed = AudioRouteSnapshot::confirmed(eq.clone(), eq).unwrap();
        assert_eq!(confirmed.state, AudioRouteState::Confirmed);
        assert!(confirmed.validate().is_ok());
    }

    #[test]
    fn transitions_uncertainty_and_inconsistent_states_are_distinct() {
        let shared = route(AudioAccess::Shared, AudioProcessing::Off, "pipewire:sink-1");
        let eq = route(
            AudioAccess::Shared,
            AudioProcessing::Equalizer,
            "pipewire:sink-1",
        );
        let transition =
            AudioRouteSnapshot::transitioning(eq.clone(), Some(shared.clone())).unwrap();
        assert_eq!(transition.state, AudioRouteState::Transitioning);
        assert_eq!(transition.confirmed, Some(shared));
        assert!(AudioRouteSnapshot::uncertain(eq.clone())
            .unwrap()
            .confirmed
            .is_none());
        assert_eq!(
            AudioRouteSnapshot::confirmed(
                eq.clone(),
                route(AudioAccess::Shared, AudioProcessing::Off, "pipewire:sink-1")
            ),
            Err(RouteContractError::RequestedAndObservedRouteDiffer)
        );
        let inconsistent = AudioRouteSnapshot {
            state: AudioRouteState::Confirmed,
            requested: eq,
            confirmed: None,
        };
        assert_eq!(
            inconsistent.validate(),
            Err(RouteContractError::InvalidSnapshotState)
        );
    }

    #[test]
    fn invalid_and_incompatible_routes_are_rejected() {
        assert_eq!(
            route(AudioAccess::Exclusive, AudioProcessing::Off, "").validate(),
            Err(RouteContractError::InvalidDeviceIdentifier)
        );
        let default_direct = AudioRoute {
            destination: AudioDestination::SystemDefault,
            access: AudioAccess::Exclusive,
            processing: AudioProcessing::Off,
        };
        assert_eq!(
            default_direct.validate(),
            Err(RouteContractError::ExclusiveSystemDefault)
        );
        assert_eq!(
            route(
                AudioAccess::Exclusive,
                AudioProcessing::Equalizer,
                "hw:Fixture,0"
            )
            .validate(),
            Err(RouteContractError::EqualizerRequiresSharedAccess)
        );
    }

    fn projection_input(
        requested: LegacyRouteInterpretation,
        applied: Option<AudioRoute>,
        dsp: RouteDspState,
    ) -> RouteProjectionInput {
        RouteProjectionInput {
            persisted: Some(requested),
            applied,
            supervisor: RouteSupervisorState::Available,
            playback_consistent: true,
            playing: false,
            direct_pcm_observed: false,
            dsp,
            operation: RouteOperationState::Stable,
            diagnostic: None,
            operation_id: None,
            generation: 7,
            platform: RoutePlatform::Linux,
            null_output_test: false,
        }
    }

    fn shared_interpretation() -> LegacyRouteInterpretation {
        interpret_legacy_route("pipewire", "default")
    }

    fn direct_interpretation() -> LegacyRouteInterpretation {
        interpret_legacy_route("alsa", "hw:CARD=Fixture,DEV=0")
    }

    #[test]
    fn linux_shared_route_is_confirmed_as_software_output_not_as_pcm_or_audible() {
        let requested = shared_interpretation();
        let value = project_audio_route(projection_input(
            requested.clone(),
            Some(requested.route.clone()),
            RouteDspState::Off,
        ));
        assert_eq!(value.operation, RouteOperationState::Stable);
        assert_eq!(
            value.route.as_ref().unwrap().state,
            AudioRouteState::Confirmed
        );
        assert_eq!(value.pcm_acquired, None);
        assert_eq!(value.exclusive_access_confirmed, None);
        assert!(value
            .evidence
            .contains(&RouteEvidence::SupervisorOutputApplied));
    }

    #[test]
    fn direct_configuration_without_an_owned_pcm_is_not_reported_as_active() {
        let requested = direct_interpretation();
        let value = project_audio_route(projection_input(
            requested.clone(),
            Some(requested.route.clone()),
            RouteDspState::Off,
        ));
        assert_eq!(
            value.route.as_ref().unwrap().state,
            AudioRouteState::Requested
        );
        assert!(value.route.as_ref().unwrap().confirmed.is_none());
        assert_eq!(value.pcm_acquired, None);
        assert_eq!(value.exclusive_access_confirmed, None);
        assert_eq!(
            value.diagnostic,
            Some(RouteDiagnostic::DirectPcmNotObserved)
        );
    }

    #[test]
    fn direct_is_confirmed_only_after_linux_reports_the_owned_pcm() {
        let requested = direct_interpretation();
        let mut input = projection_input(
            requested.clone(),
            Some(requested.route.clone()),
            RouteDspState::Off,
        );
        input.playing = true;
        input.direct_pcm_observed = true;
        let value = project_audio_route(input);
        assert_eq!(
            value.route.as_ref().unwrap().state,
            AudioRouteState::Confirmed
        );
        assert_eq!(value.pcm_acquired, Some(true));
        assert_eq!(value.exclusive_access_confirmed, Some(true));
    }

    #[test]
    fn requested_eq_is_transitioning_until_the_dsp_graph_is_verified() {
        let requested = shared_interpretation();
        let value = project_audio_route(projection_input(
            requested.clone(),
            Some(requested.route.clone()),
            RouteDspState::Preparing,
        ));
        let route = value.route.unwrap();
        assert_eq!(route.state, AudioRouteState::Transitioning);
        assert_eq!(route.requested.processing, AudioProcessing::Equalizer);
        assert_eq!(route.confirmed, Some(requested.route));
        assert_eq!(value.operation, RouteOperationState::Transitioning);
    }

    #[test]
    fn configuration_transition_keeps_the_last_confirmed_route_and_operation_id() {
        let previous = shared_interpretation();
        let requested = direct_interpretation();
        let mut input =
            projection_input(requested, Some(previous.route.clone()), RouteDspState::Off);
        input.operation = RouteOperationState::Transitioning;
        input.operation_id = Some("config:12".into());
        let value = project_audio_route(input);
        let route = value.route.unwrap();
        assert_eq!(value.operation, RouteOperationState::Transitioning);
        assert_eq!(value.operation_id.as_deref(), Some("config:12"));
        assert_eq!(route.state, AudioRouteState::Transitioning);
        assert_eq!(route.confirmed, Some(previous.route));
    }

    #[test]
    fn failed_configuration_is_not_reported_as_a_supervisor_success() {
        let requested = shared_interpretation();
        let mut input =
            projection_input(requested.clone(), Some(requested.route), RouteDspState::Off);
        input.operation = RouteOperationState::Failed;
        input.diagnostic = Some(RouteDiagnostic::ConfigurationFailed);
        let value = project_audio_route(input);
        assert_eq!(value.operation, RouteOperationState::Failed);
        assert_eq!(value.route.unwrap().state, AudioRouteState::Uncertain);
        assert_eq!(value.diagnostic, Some(RouteDiagnostic::ConfigurationFailed));
    }

    #[test]
    fn eq_is_confirmed_only_with_an_active_verified_graph_in_the_shared_path() {
        let requested = shared_interpretation();
        let mut input = projection_input(
            requested.clone(),
            Some(requested.route.clone()),
            RouteDspState::ActiveVerified,
        );
        input.operation_id = Some("dsp:4".into());
        input.generation = 8;
        let value = project_audio_route(input);
        assert_eq!(
            value.route.as_ref().unwrap().state,
            AudioRouteState::Confirmed
        );
        assert_eq!(
            value
                .route
                .as_ref()
                .unwrap()
                .confirmed
                .as_ref()
                .unwrap()
                .processing,
            AudioProcessing::Equalizer
        );
        assert!(value.evidence.contains(&RouteEvidence::DspGraphVerified));
        assert_eq!(value.pcm_acquired, None);
    }

    #[test]
    fn serialized_eq_projection_matches_the_shared_frontend_contract_fixture() {
        let requested = shared_interpretation();
        let mut input = projection_input(
            requested.clone(),
            Some(requested.route),
            RouteDspState::ActiveVerified,
        );
        input.generation = 9;
        input.operation_id = Some("dsp:4".into());
        let actual = serde_json::to_value(project_audio_route(input)).unwrap();
        let expected: Value = serde_json::from_str(include_str!(
            "../../tests/fixtures/audio-route-projection.json"
        ))
        .unwrap();
        assert_eq!(actual, expected);
    }

    #[test]
    fn unavailable_or_inconsistent_supervisor_never_confirms_a_route() {
        let requested = shared_interpretation();
        let mut input = projection_input(
            requested.clone(),
            Some(requested.route.clone()),
            RouteDspState::Off,
        );
        input.supervisor = RouteSupervisorState::Unavailable;
        let value = project_audio_route(input);
        assert_eq!(value.operation, RouteOperationState::Uncertain);
        assert_eq!(
            value.route.as_ref().unwrap().state,
            AudioRouteState::Uncertain
        );
        assert!(value.route.unwrap().confirmed.is_none());
    }

    #[test]
    fn failed_transition_and_null_output_test_are_explicitly_uncertain() {
        let requested = shared_interpretation();
        let mut failed = projection_input(
            requested.clone(),
            Some(requested.route.clone()),
            RouteDspState::Failed,
        );
        failed.operation_id = Some("dsp:5".into());
        let value = project_audio_route(failed);
        assert_eq!(value.operation, RouteOperationState::Failed);
        assert_eq!(
            value.route.as_ref().unwrap().state,
            AudioRouteState::Uncertain
        );
        assert_eq!(value.diagnostic, Some(RouteDiagnostic::DspFailed));

        let mut null_output =
            projection_input(requested.clone(), Some(requested.route), RouteDspState::Off);
        null_output.null_output_test = true;
        let value = project_audio_route(null_output);
        assert_eq!(value.operation, RouteOperationState::Uncertain);
        assert_eq!(value.applied, None);
        assert_eq!(value.diagnostic, Some(RouteDiagnostic::NullOutputTestMode));
    }

    #[test]
    fn windows_does_not_infer_wasapi_exclusivity_from_saved_settings_or_session() {
        let requested = direct_interpretation();
        let mut input = projection_input(
            requested.clone(),
            Some(requested.route.clone()),
            RouteDspState::Off,
        );
        input.platform = RoutePlatform::Windows;
        input.playing = true;
        input.direct_pcm_observed = true;
        let value = project_audio_route(input);
        assert_eq!(
            value.route.as_ref().unwrap().state,
            AudioRouteState::Requested
        );
        assert!(value.route.unwrap().confirmed.is_none());
        assert_eq!(value.pcm_acquired, None);
        assert_eq!(value.exclusive_access_confirmed, None);
        assert_eq!(
            value.diagnostic,
            Some(RouteDiagnostic::WindowsExclusiveNotObservable)
        );
    }

    #[test]
    fn unknown_legacy_output_mapping_remains_visible_but_is_not_confirmed() {
        let requested = interpret_legacy_route("future_output", "default");
        let value = project_audio_route(projection_input(
            requested.clone(),
            Some(requested.route.clone()),
            RouteDspState::Off,
        ));
        assert_eq!(
            value.route.as_ref().unwrap().state,
            AudioRouteState::Uncertain
        );
        assert_eq!(
            value.diagnostic,
            Some(RouteDiagnostic::AmbiguousLegacyConfiguration)
        );
        assert!(value
            .legacy_issues
            .contains(&LegacyRouteIssue::UnknownOutputTypeUsesLegacyMapping));
    }

    #[test]
    fn stale_route_operation_and_legacy_alsa_default_are_preserved_in_snapshot() {
        let legacy = interpret_legacy_route("alsa", "default");
        let value = project_audio_route(projection_input(
            legacy,
            Some(interpret_legacy_route("alsa", "default").route),
            RouteDspState::Off,
        ));
        assert_eq!(value.generation, 7);
        assert_eq!(value.route.unwrap().requested.access, AudioAccess::Shared);
        assert!(value
            .legacy_issues
            .contains(&LegacyRouteIssue::LegacyAlsaDefaultMapsToShared));
    }
}
