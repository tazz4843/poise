use crate::serenity_prelude as serenity;

/// Retrieves user permissions in the given channel. If unknown, returns None. If in DMs, returns
/// `Permissions::all()`.
async fn user_permissions(
    ctx: &serenity::Context,
    guild_id: Option<serenity::GuildId>,
    channel_id: serenity::ChannelId,
    user_id: serenity::UserId,
) -> Option<serenity::Permissions> {
    let guild_id = match guild_id {
        Some(x) => x,
        None => return Some(serenity::Permissions::all()), // no permission checks in DMs
    };

    let guild = match ctx.cache.guild(guild_id) {
        Some(x) => x,
        None => return None, // Guild not in cache
    };

    let channel = match guild.channels.get(&channel_id) {
        Some(serenity::Channel::Guild(channel)) => channel,
        Some(_other_channel) => {
            println!(
                "Warning: guild message was supposedly sent in a non-guild channel. Denying invocation"
            );
            return None;
        }
        None => return None,
    };

    // If member not in cache (probably because presences intent is not enabled), retrieve via HTTP
    let member = match guild.members.get(&user_id) {
        Some(x) => x.clone(),
        None => match ctx.http.get_member(guild_id.0, user_id.0).await {
            Ok(member) => member,
            Err(_) => return None,
        },
    };

    guild.user_permissions_in(channel, &member).ok()
}

/// Returns None if permissions couldn't be retrieved
async fn missing_permissions<U, E>(
    ctx: crate::Context<'_, U, E>,
    user: serenity::UserId,
    required_permissions: serenity::Permissions,
) -> Option<serenity::Permissions> {
    if required_permissions.is_empty() {
        return Some(serenity::Permissions::empty());
    }

    let permissions = user_permissions(ctx.discord(), ctx.guild_id(), ctx.channel_id(), user).await;
    match permissions {
        Some(perms) => Some(required_permissions - perms),
        None => None,
    }
}

pub async fn check_permissions_and_cooldown<'a, U, E>(
    ctx: crate::Context<'a, U, E>,
    cmd: &crate::CommandId<U, E>,
) -> Result<(), crate::FrameworkError<'a, U, E>> {
    if cmd.owners_only && !ctx.framework().options().owners.contains(&ctx.author().id) {
        return Err(crate::FrameworkError::NotAnOwner { ctx });
    }

    // Make sure that user has required permissions
    match missing_permissions(ctx, ctx.author().id, cmd.required_permissions).await {
        Some(missing_permissions) if missing_permissions.is_empty() => {}
        Some(missing_permissions) => {
            return Err(crate::FrameworkError::MissingUserPermissions {
                ctx,
                missing_permissions: Some(missing_permissions),
            })
        }
        // Better safe than sorry: when perms are unknown, restrict access
        None => {
            return Err(crate::FrameworkError::MissingUserPermissions {
                ctx,
                missing_permissions: None,
            })
        }
    }

    // Before running any pre-command checks, make sure the bot has the permissions it needs
    let bot_user_id = ctx.discord().cache.current_user_id();
    match missing_permissions(ctx, bot_user_id, cmd.required_bot_permissions).await {
        Some(missing_permissions) if missing_permissions.is_empty() => {}
        Some(missing_permissions) => {
            return Err(crate::FrameworkError::MissingBotPermissions {
                ctx,
                missing_permissions,
            })
        }
        // When in doubt, just let it run. Not getting fancy missing permissions errors is better
        // than the command not executing at all
        None => {}
    }

    // Only continue if command checks returns true
    if let Some(check) = cmd.check.or(ctx.framework().options().command_check) {
        match check(ctx).await {
            Ok(true) => {}
            Ok(false) => return Err(crate::FrameworkError::CommandCheckFailed { ctx }),
            Err(error) => {
                return Err(crate::FrameworkError::Command {
                    error,
                    ctx,
                    location: crate::CommandErrorLocation::Check,
                })
            }
        }
    }

    let cooldowns = &cmd.cooldowns;
    let remaining_cooldown = cooldowns.lock().unwrap().remaining_cooldown(ctx);
    if let Some(remaining_cooldown) = remaining_cooldown {
        return Err(crate::FrameworkError::CooldownHit {
            ctx,
            remaining_cooldown,
        });
    }
    cooldowns.lock().unwrap().start_cooldown(ctx);

    Ok(())
}
