use std::{collections::HashMap, time::Instant};

use smithay_client_toolkit::reexports::client;
use smithay_client_toolkit::{
    compositor::CompositorState,
    delegate_compositor, delegate_layer, delegate_output, delegate_registry, delegate_shm,
    output::OutputState,
    reexports::{
        calloop::{self, RegistrationToken},
        client::Proxy,
    },
    registry::RegistryState,
    shell::{
        WaylandSurface,
        wlr_layer::{Anchor, KeyboardInteractivity, Layer, LayerShell},
    },
    shm::Shm,
};
use tracing::{error, info, warn};

use wayper_lib::config::Config;

use crate::{
    map::{OutputKey, OutputMap},
    output::OutputRepr,
    wgpu_renderer::WgpuRenderer,
};

mod compositor;
mod layer_shell;
mod output;
mod registry;
mod shm;
mod utils;

pub type OutputId = u32;
/// The key should be the output id from WlOutput
pub type DrawTokens = HashMap<OutputId, RegistrationToken>;

pub struct Wayper {
    pub compositor_state: CompositorState,
    pub registry_state: RegistryState,
    pub output_state: OutputState,
    pub layer_shell: LayerShell,
    pub shm: Shm,
    pub c_queue_handle: calloop::LoopHandle<'static, Self>,
    pub draw_tokens: DrawTokens,

    pub current_profile: String,
    pub outputs: OutputMap,
    pub config: Config,
    pub socket_counter: u64,

    pub wgpu: WgpuRenderer,
}

// TODO: modularize with calloop?

impl Wayper {
    pub fn add_output(
        &mut self,
        _conn: &client::Connection,
        qh: &client::QueueHandle<Self>,
        output: client::protocol::wl_output::WlOutput,
    ) {
        let output_info = self.output_state.info(&output).expect("get info");
        let outputs_map = &mut self.outputs;

        let name = output_info.name.clone().expect("output must have name");
        tracing::Span::current().record("name", &name);

        // if output does not exist we add it
        if !outputs_map.contains_key(OutputKey::OutputName(name.clone())) {
            info!("got new_output {}", name);

            let surface = self.compositor_state.create_surface(qh);
            let layer = self.layer_shell.create_layer_surface(
                qh,
                surface.clone(),
                Layer::Background,
                Some("wayper"),
                Some(&output),
            );

            // additional layer configuration
            layer.set_layer(Layer::Background);
            layer.set_size(0, 0);
            layer.set_exclusive_zone(-1);
            layer.set_anchor(Anchor::all());
            layer.set_keyboard_interactivity(KeyboardInteractivity::None);

            // commit the layer
            layer.commit();

            self.wgpu
                .new_surface(
                    name.clone(),
                    _conn.backend().display_ptr(),
                    layer.wl_surface().id().as_ptr(),
                )
                .unwrap();

            // no config no problem
            let output_config = match self
                .config
                .get_output_config(&*self.current_profile, name.as_str())
            {
                Ok(config) => Some(config),
                Err(e) => {
                    error!("Unable to get config for output: {e}");
                    None
                }
            };

            let img_list = utils::get_img_list(output_config.as_ref());

            outputs_map.insert(
                name.clone(),
                surface.id(),
                output.id(),
                OutputRepr {
                    output_name: name.clone(),
                    _wl_repr: output,
                    output_info,
                    output_config,
                    dimensions: None,
                    _scale_factor: 1,
                    _surface: Some(surface),
                    _layer: layer,
                    buffer: None,
                    first_configure: true,
                    ping_draw: None,
                    img_list,
                    index: 0,
                    visible: true,
                    should_next: false,
                    last_render_instant: Instant::now(),
                },
            );
        } else {
            warn!("we had this output {name} earlier, skipping....");
        }
    }
    pub fn change_profile<P>(&mut self, profile: P) -> color_eyre::Result<String>
    where
        P: Into<Option<String>>,
    {
        let profile =
            Into::<Option<String>>::into(profile).unwrap_or(self.config.default_profile.clone());
        if !&self
            .config
            .profiles
            .profiles()
            .contains(&profile.to_string())
        {
            return Err(color_eyre::eyre::eyre!("Profile does not exist"));
        }

        if profile == self.current_profile {
            warn!(
                "Not changing to currently active profile {}",
                self.current_profile
            );
            return Ok(profile);
        }

        info!("Changing current profile to: \"{profile}\"");

        // set the profile
        self.current_profile = profile.to_string();

        // refresh the img list
        for output in self.outputs.iter() {
            let output_name = output.lock().unwrap().output_name.clone();

            let output_config = self
                .config
                .get_output_config(profile.as_str(), &*output_name)?;

            let mut output = output.lock().unwrap();
            output.img_list = utils::get_img_list(Some(&output_config));
            output.index = 0;
            output.output_config = Some(output_config);
            if let Some(ping_draw) = output.ping_draw.as_ref() {
                ping_draw.ping();
            } else {
                error!("ping draw does not exist, did output configure fail?");
            }
        }

        Ok(profile)
    }
}

delegate_compositor!(Wayper);
delegate_output!(Wayper);
delegate_layer!(Wayper);
delegate_registry!(Wayper);
delegate_shm!(Wayper);
