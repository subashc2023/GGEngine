#version 450

layout(set = 0, binding = 0) uniform CameraUBO {
    mat4 view;
    mat4 projection;
    mat4 viewProjection;
} camera;

layout(location = 0) in vec3 aPosition;   // World-space position (CPU-transformed)
layout(location = 1) in vec2 aTexCoord;
layout(location = 2) in vec4 aColor;

layout(location = 0) out vec2 vTexCoord;
layout(location = 1) out vec4 vColor;

void main() {
    gl_Position = camera.viewProjection * vec4(aPosition, 1.0);
    vTexCoord = aTexCoord;
    vColor = aColor;
}
