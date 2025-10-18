use smithay_client_toolkit::{
    output::{OutputHandler, OutputState},
    reexports::client::{self, Proxy},
};
use tracing::{debug, error, info, instrument, trace};

use crate::{handlers::Wayper, map::OutputKey};

impl OutputHandler for Wayper {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.output_state
    }

    #[instrument(skip_all, fields(name))]
    fn new_output(
        &mut self,
        _conn: &client::Connection,
        qh: &client::QueueHandle<Self>,
        output: client::protocol::wl_output::WlOutput,
    ) {
        debug!("received new_output {} on output handler", output.id());
        self.add_output(_conn, qh, output);
    }

    fn update_output(
        &mut self,
        conn: &client::Connection,
        qh: &client::QueueHandle<Self>,
        output: client::protocol::wl_output::WlOutput,
    ) {
        // TODO: implement this because usecase is found - when an output is added
        debug!("received update_output for output {}", output.id());
        self.add_output(conn, qh, output);
    }

    fn output_destroyed(
        &mut self,
        _conn: &client::Connection,
        _qh: &client::QueueHandle<Self>,
        output: client::protocol::wl_output::WlOutput,
    ) {
        let info = self.output_state.info(&output).expect("output has info");
        output.release();
        let name = info.name.expect("output has name");

        let _removed = self.outputs.remove(OutputKey::OutputName(name.clone()));
        info!("output {name} was removed");
        match self.draw_tokens.remove_entry(&info.id) {
            Some((_, token)) => {
                self.c_queue_handle.remove(token);
                trace!("removed timer for {name}");
            }
            None => {
                error!("failed to remove timer_token entry");
            }
        }
    }
}
