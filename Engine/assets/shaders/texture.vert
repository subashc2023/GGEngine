#version 450

layout(set = 0, binding = 0) uniform CameraUBO {
    mat4 view;
    mat4 projection;
    mat4 viewProjection;
} camera;

layout(push_constant) uniform PushConstants {
    mat4 model;
} push;

layout(location = 0) in vec3 aPosition;
layout(location = 1) in vec2 aTexCoord;

layout(location = 0) out vec2 vTexCoord;

void main() {
    gl_Position = camera.viewProjection * push.model * vec4(aPosition, 1.0);
    vTexCoord = aTexCoord;
}
