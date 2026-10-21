//! Turns the render data the engine extracts each frame into images: render graph, passes,
//! materials, lighting and post-processing, built on `gg-gpu`.
//!
//! It knows nothing about the scene, ECS or gameplay. It receives plain data, so rendering
//! can run on its own thread, a frame behind simulation.
