use smithay_client_toolkit::shm::ShmHandler;

use crate::handlers::Wayper;

impl ShmHandler for Wayper {
    fn shm_state(&mut self) -> &mut smithay_client_toolkit::shm::Shm {
        &mut self.shm
    }
}
