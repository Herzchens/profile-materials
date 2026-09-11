use twilight_model::gateway::{
    payload::incoming::PresenceUpdate,
    presence::{Activity, ActivityType, Status},
};
use twilight_model::id::{
    Id,
    marker::{GuildMarker, UserMarker},
};

use crate::state::{
    ActivityAssetsSnapshot, ActivityKind, ActivitySnapshot, ActivityTimestampsSnapshot,
    ClientStatusSnapshot, PresenceData, PresenceStatus,
};

pub fn is_target(
    presence: &PresenceUpdate,
    target_user_id: Id<UserMarker>,
    target_guild_id: Id<GuildMarker>,
) -> bool {
    presence.user.id() == target_user_id && presence.guild_id == target_guild_id
}

pub fn normalize_presence(presence: &PresenceUpdate) -> PresenceData {
    PresenceData {
        activities: presence.activities.iter().map(normalize_activity).collect(),
        client_status: ClientStatusSnapshot {
            desktop: presence.client_status.desktop.map(normalize_status),
            mobile: presence.client_status.mobile.map(normalize_status),
            web: presence.client_status.web.map(normalize_status),
        },
        status: normalize_status(presence.status),
    }
}

fn normalize_activity(activity: &Activity) -> ActivitySnapshot {
    ActivitySnapshot {
        application_id: activity.application_id.map(|id| id.get().to_string()),
        assets: activity
            .assets
            .as_ref()
            .map(|assets| ActivityAssetsSnapshot {
                large_image: assets.large_image.clone(),
                large_text: assets.large_text.clone(),
                small_image: assets.small_image.clone(),
                small_text: assets.small_text.clone(),
            }),
        details: activity.details.clone(),
        kind: match activity.kind {
            ActivityType::Competing => ActivityKind::Competing,
            ActivityType::Custom => ActivityKind::Custom,
            ActivityType::Listening => ActivityKind::Listening,
            ActivityType::Playing => ActivityKind::Playing,
            ActivityType::Streaming => ActivityKind::Streaming,
            ActivityType::Watching => ActivityKind::Watching,
            ActivityType::Unknown(value) => ActivityKind::Unknown(value),
            _ => ActivityKind::Unknown(u8::MAX),
        },
        name: activity.name.clone(),
        state: activity.state.clone(),
        timestamps: activity
            .timestamps
            .as_ref()
            .map(|timestamps| ActivityTimestampsSnapshot {
                end: timestamps.end,
                start: timestamps.start,
            }),
    }
}

fn normalize_status(status: Status) -> PresenceStatus {
    match status {
        Status::DoNotDisturb => PresenceStatus::DoNotDisturb,
        Status::Idle => PresenceStatus::Idle,
        Status::Invisible => PresenceStatus::Invisible,
        Status::Offline => PresenceStatus::Offline,
        Status::Online => PresenceStatus::Online,
    }
}

#[cfg(test)]
mod tests {
    use twilight_model::{
        gateway::{
            payload::incoming::PresenceUpdate,
            presence::{
                Activity, ActivityAssets, ActivitySecrets, ActivityTimestamps, ActivityType,
                ClientStatus, Presence, Status, UserOrId,
            },
        },
        id::{
            Id,
            marker::{ApplicationMarker, GuildMarker, UserMarker},
        },
    };

    use crate::state::{ActivityKind, PresenceStatus};

    use super::{is_target, normalize_presence};

    fn sample_presence() -> PresenceUpdate {
        PresenceUpdate(Presence {
            activities: vec![Activity {
                application_id: Some(Id::<ApplicationMarker>::new(333)),
                assets: Some(ActivityAssets {
                    large_image: Some("large".to_owned()),
                    large_text: Some("Code".to_owned()),
                    small_image: Some("small".to_owned()),
                    small_text: Some("Rust".to_owned()),
                }),
                buttons: Vec::new(),
                created_at: Some(123),
                details: Some("Editing".to_owned()),
                emoji: None,
                flags: None,
                id: Some("activity".to_owned()),
                instance: Some(true),
                kind: ActivityType::Playing,
                name: "Visual Studio Code".to_owned(),
                party: None,
                secrets: Some(ActivitySecrets {
                    join: Some("do-not-expose".to_owned()),
                    match_: None,
                    spectate: None,
                }),
                state: Some("main.rs".to_owned()),
                timestamps: Some(ActivityTimestamps {
                    end: Some(200),
                    start: Some(100),
                }),
                url: None,
            }],
            client_status: ClientStatus {
                desktop: Some(Status::Online),
                mobile: None,
                web: None,
            },
            guild_id: Id::<GuildMarker>::new(222),
            status: Status::Online,
            user: UserOrId::UserId {
                id: Id::<UserMarker>::new(111),
            },
        })
    }

    #[test]
    fn filters_by_both_user_and_guild() {
        let presence = sample_presence();

        assert!(is_target(
            &presence,
            Id::<UserMarker>::new(111),
            Id::<GuildMarker>::new(222)
        ));
        assert!(!is_target(
            &presence,
            Id::<UserMarker>::new(112),
            Id::<GuildMarker>::new(222)
        ));
        assert!(!is_target(
            &presence,
            Id::<UserMarker>::new(111),
            Id::<GuildMarker>::new(223)
        ));
    }

    #[test]
    fn normalizes_only_allow_listed_activity_fields() {
        let normalized = normalize_presence(&sample_presence());

        assert_eq!(normalized.status, PresenceStatus::Online);
        assert_eq!(normalized.activities.len(), 1);
        assert_eq!(normalized.activities[0].kind, ActivityKind::Playing);
        assert_eq!(
            normalized.activities[0].application_id.as_deref(),
            Some("333")
        );
        assert_eq!(normalized.activities[0].details.as_deref(), Some("Editing"));
        assert_eq!(normalized.activities[0].state.as_deref(), Some("main.rs"));
        assert_eq!(
            normalized.activities[0].timestamps.as_ref().unwrap().start,
            Some(100)
        );
    }
}
