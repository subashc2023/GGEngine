//! What builds the arena and what its ticks run: steering, collecting and placing shards.

use std::time::Duration;

use gg_engine::math::DVec3;
use gg_engine::prelude::*;
use gg_engine::{BodyType, Camera, Circle, CircleCollider2D, Name, Owner, Physics2D, RigidBody2D};

use crate::components::{Match, Player, Shard};
use crate::{
    ARENA, COLORS, DASH_SPEED, GRIP, PLAYER_SIZE, PLAYER_Z, SHARD_SIZE, SHARD_Z, SHARDS, SPEED,
    STARTS, Sounds, VIEW_HEIGHT, WINS_AT,
};

/// Builds what the arena's scene doesn't hold: the camera, the match, the players and the first
/// shards. No gravity, as the arena is seen from above.
pub(crate) fn build(world: &mut World) {
    let mut physics = Physics2D::default();
    physics.gravity = Vec2::ZERO;
    world.insert_resource(physics);
    let camera = Camera {
        projection: Projection::Orthographic {
            height: VIEW_HEIGHT,
            near: 0.1,
            far: 100.0,
        },
        primary: true,
    };
    let above = Transform {
        translation: DVec3::Z * 10.0,
        ..Transform::IDENTITY
    };
    world.spawn((above, camera, Name("Camera".to_owned())));
    world.spawn((
        Match {
            scores: [0; 2],
            wins: [0; 2],
            random: 0x5_4a2d_5eed,
        },
        Name("Match".to_owned()),
    ));
    for (index, start) in STARTS.into_iter().enumerate() {
        let at = Transform {
            translation: start.extend(PLAYER_Z).as_dvec3(),
            scale: Vec3::splat(PLAYER_SIZE),
            ..Transform::IDENTITY
        };
        let body = RigidBody2D {
            body_type: BodyType::Dynamic,
            fixed_rotation: true,
            ..RigidBody2D::default()
        };
        let collider = CircleCollider2D {
            restitution: 0.6,
            friction: 0.0,
            ..CircleCollider2D::default()
        };
        let disc = Circle {
            color: Vec3::from(COLORS[index]).extend(1.0),
            ..Circle::default()
        };
        let player = Player {
            index: index as u8,
            heading: Vec2::ZERO,
            dash: 0,
            cooldown: 0,
        };
        let name = Name(format!("Player {}", index + 1));
        let owner = Owner {
            player: index as u8,
        };
        world.spawn((at, body, collider, disc, player, owner, name));
    }
    for _ in 0..SHARDS {
        let at = Transform {
            scale: Vec3::new(SHARD_SIZE, SHARD_SIZE, 1.0) * std::f32::consts::FRAC_1_SQRT_2,
            ..Transform::IDENTITY
        };
        let sprite = Sprite {
            color: Vec4::new(0.95, 0.85, 0.3, 1.0),
            ..Sprite::default()
        };
        let shard = world.spawn((at, sprite, Shard { angle: 0.0 }));
        // Its spin, which its script declares.
        world
            .add_default(shard, "Spin")
            .expect("the game's scripts loaded");
        place(world, shard);
    }
}

/// Moves `shard` to where the match's random numbers say next, clear of the players.
fn place(world: &mut World, shard: Entity) {
    let players: Vec<Vec2> = world
        .query::<(&Player, &Transform)>()
        .map(|(_, at)| at.translation.truncate().as_vec2())
        .collect();
    let Some(found) = world.query_mut::<&mut Match>().next() else {
        return;
    };
    let mut at = Vec2::ZERO;
    // Up to 8 tries for a place clear of the players, then the last.
    for _ in 0..8 {
        let inside = ARENA - Vec2::splat(1.0);
        let x = (unit(&mut found.random) * 2.0 - 1.0) * inside.x;
        let y = (unit(&mut found.random) * 2.0 - 1.0) * inside.y;
        at = Vec2::new(x, y);
        if players.iter().all(|player| player.distance(at) > 1.5) {
            break;
        }
    }
    if let Some(transform) = world.get_mut::<Transform>(shard) {
        transform.translation = at.extend(SHARD_Z).as_dvec3();
    }
}

/// The next of `random`'s numbers, SplitMix64's, from 0 up to 1, in steps of 2⁻²⁴.
fn unit(random: &mut u64) -> f32 {
    *random = random.wrapping_add(0x9e37_79b9_7f4a_7c15);
    let mut z = *random;
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^= z >> 31;
    (z >> 40) as f32 / (1 << 24) as f32
}

/// Moves each player's velocity, over `dt`, toward its heading at its speed, faster while it
/// dashes, and counts its dash down.
pub(crate) fn steer(world: &mut World, dt: Duration) {
    let dt = dt.as_secs_f32();
    for (player, body) in world.query_mut::<(&mut Player, &mut RigidBody2D)>() {
        let speed = if player.dash > 0 {
            SPEED * DASH_SPEED
        } else {
            SPEED
        };
        let wanted = player.heading * speed;
        // A dash goes at once; otherwise the disc slides toward where it's steered.
        let grip = if player.dash > 0 {
            1.0
        } else {
            (GRIP * dt).min(1.0)
        };
        body.velocity += (wanted - body.velocity) * grip;
        player.dash = player.dash.saturating_sub(1);
        player.cooldown = player.cooldown.saturating_sub(1);
    }
}

/// Scores each shard a player touches and places it again, chiming `sounds`' collect where it was.
/// The shard that wins a match gives it to its player and starts another, with the players back at
/// their starts, the rest touched that tick left, and `sounds`' fanfare.
pub(crate) fn collect(world: &mut World, sounds: Option<Sounds>) {
    let players: Vec<(u8, Vec2)> = world
        .query::<(&Player, &Transform)>()
        .map(|(player, at)| (player.index, at.translation.truncate().as_vec2()))
        .collect();
    let touched: Vec<(Entity, u8)> = world
        .query::<(Entity, &Shard, &Transform)>()
        .filter_map(|(shard, _, at)| {
            let at = at.translation.truncate().as_vec2();
            let reach = (PLAYER_SIZE + SHARD_SIZE) / 2.0;
            let by = players
                .iter()
                .find(|(_, player)| player.distance(at) < reach);
            by.map(|&(index, _)| (shard, index))
        })
        .collect();
    let mut won = None;
    for (shard, index) in touched {
        if won.is_some() {
            break;
        }
        if let Some(found) = world.query_mut::<&mut Match>().next() {
            let index = usize::from(index);
            found.scores[index] += 1;
            if found.scores[index] >= WINS_AT {
                found.wins[index] += 1;
                found.scores = [0; 2];
                won = Some(index);
            }
        }
        let at = world.get::<Transform>(shard).map(|at| at.translation);
        if let (Some(sounds), Some(at)) = (sounds, at) {
            world.play_sound(sounds.collect, Place::At(at));
        }
        place(world, shard);
    }
    if won.is_some() {
        to_starts(world);
        if let Some(sounds) = sounds {
            world.play_sound(sounds.win, Place::Nowhere);
        }
    }
}

/// Puts each player back at its start, at rest.
pub(crate) fn to_starts(world: &mut World) {
    let players = world.query_mut::<(&Player, &mut Transform, &mut RigidBody2D)>();
    for (player, at, body) in players {
        at.translation = STARTS[usize::from(player.index)]
            .extend(PLAYER_Z)
            .as_dvec3();
        body.velocity = Vec2::ZERO;
    }
}
