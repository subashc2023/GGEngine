#version 450

layout(set = 0, binding = 0) uniform CameraUBO {
    mat4 view;
    mat4 projection;
    mat4 viewProjection;
} camera;

layout(location = 0) in vec3 aPosition;

void main() {
    gl_Position = camera.viewProjection * vec4(aPosition, 1.0);
}
