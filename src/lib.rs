extern crate cgmath;
extern crate froggy;
#[macro_use]
extern crate gfx;
extern crate winit;
// OpenGL
#[cfg(feature = "opengl")]
extern crate gfx_device_gl as back;
#[cfg(feature = "opengl")]
extern crate gfx_window_glutin;
#[cfg(feature = "opengl")]
extern crate glutin;

use cgmath::prelude::*;
use cgmath::Transform as Transform_;
use gfx::traits::{Device, FactoryExt};
use std::ops;
use std::sync::mpsc;


pub type Position = cgmath::Point3<f32>;
pub type Normal = cgmath::Vector3<f32>;
pub type Orientation = cgmath::Quaternion<f32>;
pub type Transform = cgmath::Decomposed<Normal, Orientation>;
pub type ColorFormat = gfx::format::Srgba8;
pub type DepthFormat = gfx::format::DepthStencil;
type SceneId = usize;

gfx_vertex_struct!(Vertex {
    pos: [f32; 4] = "a_Position",
});

gfx_pipeline!(pipe {
    vbuf: gfx::VertexBuffer<Vertex> = (),
    mx_vp: gfx::Global<[[f32; 4]; 4]> = "u_ViewProj",
    mx_world: gfx::Global<[[f32; 4]; 4]> = "u_World",
    color: gfx::Global<[f32; 4]> = "u_Color",
    out_color: gfx::RenderTarget<ColorFormat> = "Target0",
});

const LINE_VS: &'static [u8] = b"
    #version 150 core
    in vec4 a_Position;
    uniform mat4 u_ViewProj;
    uniform mat4 u_World;
    void main() {
        gl_Position = u_ViewProj * u_World * a_Position;
    }
";
const LINE_FS: &'static [u8] = b"
    #version 150 core
    uniform vec4 u_Color;
    void main() {
        gl_FragColor = u_Color;
    }
";

pub struct Factory {
    graphics: back::Factory,
    scene_id: SceneId,
}

pub struct Renderer {
    device: back::Device,
    encoder: gfx::Encoder<back::Resources, back::CommandBuffer>,
    out_color: gfx::handle::RenderTargetView<back::Resources, ColorFormat>,
    out_depth: gfx::handle::DepthStencilView<back::Resources, DepthFormat>,
    pso_line: gfx::PipelineState<back::Resources, pipe::Meta>,
    size: (u32, u32),
    #[cfg(feature = "opengl")]
    window: glutin::Window,
}

#[cfg(feature = "opengl")]
impl Renderer {
    pub fn new(builder: glutin::WindowBuilder, event_loop: &glutin::EventsLoop)
               -> (Renderer, Factory) {
        let (window, device, mut gl_factory, color, depth) =
            gfx_window_glutin::init(builder, event_loop);
        let renderer = Renderer {
            device: device,
            encoder: gl_factory.create_command_buffer().into(),
            out_color: color,
            out_depth: depth,
            pso_line: {
                let prog = gl_factory.link_program(LINE_VS, LINE_FS).unwrap();
                let rast = gfx::state::Rasterizer::new_fill();
                gl_factory.create_pipeline_from_program(&prog,
                    gfx::Primitive::LineStrip, rast, pipe::new()
                ).unwrap()
            },
            size: window.get_inner_size_pixels().unwrap(),
            window: window,
        };
        let factory = Factory {
            graphics: gl_factory,
            scene_id: 0,
        };
        (renderer, factory)
    }

    pub fn resize(&mut self) {
        self.size = self.window.get_inner_size_pixels().unwrap();
        gfx_window_glutin::update_views(&self.window, &mut self.out_color, &mut self.out_depth);
    }
}

pub trait Camera {
    fn to_view_proj(&self) -> cgmath::Matrix4<f32>;
}

pub struct PerspectiveCamera {
    pub projection: cgmath::PerspectiveFov<f32>,
    pub position: Position,
    pub orientation: Orientation,
}

impl PerspectiveCamera {
    pub fn new(fov: f32, aspect: f32, near: f32, far: f32) -> PerspectiveCamera {
        PerspectiveCamera {
            projection: cgmath::PerspectiveFov {
                fovy: cgmath::Deg(fov).into(),
                aspect: aspect,
                near: near,
                far: far,
            },
            position: Position::origin(),
            orientation: Orientation::one(),
        }
    }

    pub fn look_at(&mut self, target: cgmath::Point3<f32>) {
        let dir = (self.position - target).normalize();
        let z = cgmath::Vector3::unit_z();
        let up = if dir.dot(z).abs() < 0.99 { z } else {
            cgmath::Vector3::unit_y()
        };
        self.orientation = Orientation::look_at(dir, up);
    }
}

impl Camera for PerspectiveCamera {
    fn to_view_proj(&self) -> cgmath::Matrix4<f32> {
        let mx_proj = cgmath::perspective(self.projection.fovy,
            self.projection.aspect, self.projection.near, self.projection.far);
        let transform = cgmath::Decomposed {
            disp: self.position.to_vec(),
            rot: self.orientation,
            scale: 1.0,
        };

        let mx_view = cgmath::Matrix4::from(transform.inverse_transform().unwrap());
        mx_proj * mx_view
    }
}

#[derive(Clone)]
pub struct Geometry {
    pub vertices: Vec<Position>,
    pub normals: Vec<Normal>,
    pub faces: Vec<[u16; 3]>,
    pub is_dynamic: bool,
}

impl Geometry {
    pub fn new() -> Geometry {
        Geometry {
            vertices: Vec::new(),
            normals: Vec::new(),
            faces: Vec::new(),
            is_dynamic: false,
        }
    }
    pub fn from_vertices(verts: Vec<Position>) -> Geometry {
        Geometry {
            vertices: verts,
            .. Geometry::new()
        }
    }
}


enum Message {
    SetTransform(froggy::WeakPointer<Node>, Transform),
    SetMaterial(froggy::WeakPointer<Visual>, Material),
    //Delete,
}

type NodePtr = froggy::Pointer<Node>;
type VisualPtr = froggy::Pointer<Visual>;

pub type Color = u32;

#[derive(Clone)]
pub enum Material {
    LineBasic { color: Color },
}

struct SceneLink<V> {
    id: SceneId,
    node: NodePtr,
    visual: V,
    tx: mpsc::Sender<Message>,
}

pub struct Object {
    transform: Transform,
    scenes: Vec<SceneLink<()>>,
}

pub struct VisualObject {
    visible: bool,
    transform: Transform,
    material: Material,
    gpu_data: GpuData,
    scenes: Vec<SceneLink<VisualPtr>>,
}

pub struct TransformProxy<'a> {
    value: &'a mut Transform,
    links: &'a [SceneLink<()>],
}

impl<'a> ops::Deref for TransformProxy<'a> {
    type Target = Transform;
    fn deref(&self) -> &Self::Target {
        self.value
    }
}

impl<'a> ops::DerefMut for TransformProxy<'a> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.value
    }
}

impl<'a> Drop for TransformProxy<'a> {
    fn drop(&mut self) {
        for link in self.links {
            let msg = Message::SetTransform(link.node.downgrade(), self.value.clone());
            let _ = link.tx.send(msg);
        }
    }
}

pub struct TransformProxyVisual<'a> {
    value: &'a mut Transform,
    links: &'a [SceneLink<VisualPtr>],
}

impl<'a> ops::Deref for TransformProxyVisual<'a> {
    type Target = Transform;
    fn deref(&self) -> &Self::Target {
        self.value
    }
}

impl<'a> ops::DerefMut for TransformProxyVisual<'a> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.value
    }
}

impl<'a> Drop for TransformProxyVisual<'a> {
    fn drop(&mut self) {
        for link in self.links {
            let msg = Message::SetTransform(link.node.downgrade(), self.value.clone());
            let _ = link.tx.send(msg);
        }
    }
}

pub struct MaterialProxy<'a> {
    value: &'a mut Material,
    links: &'a [SceneLink<VisualPtr>],
}

impl<'a> ops::Deref for MaterialProxy<'a> {
    type Target = Material;
    fn deref(&self) -> &Self::Target {
        self.value
    }
}

impl<'a> ops::DerefMut for MaterialProxy<'a> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.value
    }
}

impl<'a> Drop for MaterialProxy<'a> {
    fn drop(&mut self) {
        for link in self.links {
            let msg = Message::SetMaterial(link.visual.downgrade(), self.value.clone());
            let _ = link.tx.send(msg);
        }
    }
}

impl Object {
    fn new() -> Self {
        Object {
            transform: Transform::one(),
            scenes: Vec::with_capacity(1),
        }
    }

    fn get_scene(&self, id: SceneId) -> Option<&SceneLink<()>> {
        self.scenes.iter().find(|link| link.id == id)
    }

    pub fn transform(&self) -> &Transform {
        &self.transform
    }

    pub fn transform_mut(&mut self) -> TransformProxy {
        TransformProxy {
            value: &mut self.transform,
            links: &self.scenes,
        }
    }

    pub fn attach(&mut self, scene: &mut Scene, group: Option<&Group>) {
        assert!(self.get_scene(scene.unique_id).is_none(),
            "Object is already in the scene");
        let node_ptr = scene.make_node(self.transform.clone(), group);
        self.scenes.push(SceneLink {
            id: scene.unique_id,
            node: node_ptr,
            visual: (),
            tx: scene.message_tx.clone(),
        });
    }
}

impl VisualObject {
    fn new(material: Material, gpu_data: GpuData) -> Self {
        VisualObject {
            visible: true,
            transform: Transform::one(),
            material: material,
            gpu_data: gpu_data,
            scenes: Vec::with_capacity(1),
        }
    }

    fn get_scene(&self, id: SceneId) -> Option<&SceneLink<VisualPtr>> {
        self.scenes.iter().find(|link| link.id == id)
    }

    pub fn transform(&self) -> &Transform {
        &self.transform
    }

    pub fn transform_mut(&mut self) -> TransformProxyVisual {
        TransformProxyVisual {
            value: &mut self.transform,
            links: &self.scenes,
        }
    }

    pub fn material(&self) -> &Material {
        &self.material
    }

    pub fn material_mut(&mut self) -> MaterialProxy {
        MaterialProxy {
            value: &mut self.material,
            links: &self.scenes,
        }
    }

    pub fn attach(&mut self, scene: &mut Scene, group: Option<&Group>) {
        assert!(self.get_scene(scene.unique_id).is_none(),
            "VisualObject is already in the scene");
        let node_ptr = scene.make_node(self.transform.clone(), group);
        let visual_ptr = scene.visuals.create(Visual {
            material: self.material.clone(),
            gpu_data: self.gpu_data.clone(),
            node: node_ptr.clone(),
        });
        self.scenes.push(SceneLink {
            id: scene.unique_id,
            node: node_ptr,
            visual: visual_ptr,
            tx: scene.message_tx.clone(),
        });
    }
}

pub struct Line {
    object: VisualObject,
    _geometry: Option<Geometry>,
}

impl ops::Deref for Line {
    type Target = VisualObject;
    fn deref(&self) -> &Self::Target {
        &self.object
    }
}

impl ops::DerefMut for Line {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.object
    }
}

pub struct Group {
    object: Object,
}

impl ops::Deref for Group {
    type Target = Object;
    fn deref(&self) -> &Self::Target {
        &self.object
    }
}

impl ops::DerefMut for Group {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.object
    }
}


struct Node {
    local: Transform,
    world: Transform,
    parent: Option<NodePtr>,
}

#[derive(Clone)]
struct GpuData {
    slice: gfx::Slice<back::Resources>,
    vertices: gfx::handle::Buffer<back::Resources, Vertex>,
}

struct Visual {
    material: Material,
    gpu_data: GpuData,
    node: NodePtr,
}

pub struct Scene {
    nodes: froggy::Storage<Node>,
    visuals: froggy::Storage<Visual>,
    unique_id: SceneId,
    message_tx: mpsc::Sender<Message>,
    message_rx: mpsc::Receiver<Message>,
}

impl Scene {
    fn make_node(&mut self, transform: Transform, group: Option<&Group>) -> NodePtr {
        let parent = group.map(|g| {
            g.get_scene(self.unique_id)
             .expect("Parent group is not in the scene")
             .node.clone()
        });
        self.nodes.create(Node {
            local: transform,
            world: Transform::one(),
            parent: parent,
        })
    }

    pub fn process_messages(&mut self) {
        while let Ok(message) = self.message_rx.try_recv() {
            match message {
                Message::SetTransform(pnode, transform) => {
                    if let Ok(ref ptr) = pnode.upgrade() {
                        self.nodes[ptr].local = transform;
                    }
                }
                Message::SetMaterial(pvisual, material) => {
                    if let Ok(ref ptr) = pvisual.upgrade() {
                        self.visuals[ptr].material = material;
                    }
                }
            }
        }
    }

    pub fn compute_transforms(&mut self) {
        let mut cursor = self.nodes.cursor();
        while let Some(mut item) = cursor.next() {
            item.world = match item.parent {
                Some(ref parent) => item.look_back(parent).unwrap().world.concat(&item.local),
                None => item.local,
            };
        }
    }

    pub fn update(&mut self) {
        self.process_messages();
        self.compute_transforms();
    }
}


impl Factory {
    pub fn scene(&mut self) -> Scene {
        self.scene_id += 1;
        let (tx, rx) = mpsc::channel();
        Scene {
            nodes: froggy::Storage::new(),
            visuals: froggy::Storage::new(),
            unique_id: self.scene_id,
            message_tx: tx,
            message_rx: rx,
        }
    }

    pub fn group(&mut self) -> Group {
        Group {
            object: Object::new(),
        }
    }

    pub fn line(&mut self, geom: Geometry, mat: Material) -> Line {
        let vertices: Vec<_> = geom.vertices.iter().map(|v| Vertex {
            pos: [v.x, v.y, v.z, 1.0],
        }).collect();
        //TODO: dynamic geometry
        let (vbuf, slice) = self.graphics.create_vertex_buffer_with_slice(&vertices, ());
        Line {
            object: VisualObject::new(mat, GpuData {
                slice: slice,
                vertices: vbuf,
            }),
            _geometry: if geom.is_dynamic { Some(geom) } else { None },
        }
    }

    // pub fn update(&self, ) //TODO: update dynamic geometry
}


impl Renderer {
    pub fn get_aspect(&self) -> f32 {
        self.size.0 as f32 / self.size.1 as f32
    }

    pub fn render<C: Camera>(&mut self, scene: &Scene, cam: &C) {
        self.device.cleanup();
        self.encoder.clear(&self.out_color, [0.0, 0.0, 0.0, 1.0]);
        self.encoder.clear_depth(&self.out_depth, 1.0);

        let mx_vp = cam.to_view_proj();
        for visual in &scene.visuals {
            let color = match visual.material {
                Material::LineBasic { color } => {
                    [((color>>16)&0xFF) as f32 / 255.0,
                     ((color>>8) &0xFF) as f32 / 255.0,
                     (color&0xFF) as f32 / 255.0,
                     1.0]
                },
            };
            let mx_world = cgmath::Matrix4::from(scene.nodes[&visual.node].world);
            let data = pipe::Data {
                vbuf: visual.gpu_data.vertices.clone(),
                mx_vp: mx_vp.into(),
                mx_world: mx_world.into(),
                color: color,
                out_color: self.out_color.clone(),
            };
            self.encoder.draw(&visual.gpu_data.slice, &self.pso_line, &data);
        }

        self.encoder.flush(&mut self.device);
        self.window.swap_buffers().unwrap();
    }
}
