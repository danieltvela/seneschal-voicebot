#[cfg(feature = "cuda")]
use ort::ep::cuda::CUDA;
#[cfg(feature = "coreml")]
use ort::ep::coreml::CoreML;
#[cfg(any(not(feature = "cuda"), feature = "coreml"))]
use ort::ep::cpu::CPU;
use ort::session::builder::SessionBuilder;
use ort::session::Session;

pub trait OrtBase {
    fn load_model(&mut self, model_path: String) -> Result<(), String> {
        self.load_model_with_device(model_path, None)
    }

    /// Like [`OrtBase::load_model`], but pins the CUDA execution provider to
    /// `device_id` when built with the `cuda` feature (seneschal patch, issue #14).
    /// `None` keeps the ONNX Runtime default (device 0).
    fn load_model_with_device(
        &mut self,
        model_path: String,
        #[cfg_attr(not(feature = "cuda"), allow(unused_variables))] device_id: Option<i32>,
    ) -> Result<(), String> {
        #[cfg(feature = "cuda")]
        let providers = [CUDA::default()
            .with_device_id(device_id.unwrap_or(0))
            .build()];

        #[cfg(feature = "coreml")]
        let providers = [
            CoreML::default().build(),
            CPU::default().build()
        ];

        #[cfg(all(not(feature = "cuda"), not(feature = "coreml")))]
        let providers = [CPU::default().build()];

        match SessionBuilder::new() {
            Ok(builder) => {
                let session = builder
                    .with_execution_providers(providers)
                    .map_err(|e| format!("Failed to build session: {}", e))?
                    .commit_from_file(model_path)
                    .map_err(|e| format!("Failed to commit from file: {}", e))?;
                self.set_sess(session);
                Ok(())
            }
            Err(e) => Err(format!("Failed to create session builder: {}", e)),
        }
    }

    fn print_info(&self) {
        if let Some(session) = self.sess() {
            tracing::debug!("Input names:");
            for input in session.inputs() {
                tracing::debug!("  - {}", input.name());
            }
            tracing::debug!("Output names:");
            for output in session.outputs() {
                tracing::debug!("  - {}", output.name());
            }

            #[cfg(feature = "cuda")]
            tracing::debug!("Configured with: CUDA execution provider");

            #[cfg(feature = "coreml")]
            tracing::debug!("Configured with: CoreML execution provider");

            #[cfg(all(not(feature = "cuda"), not(feature = "coreml")))]
            tracing::debug!("Configured with: CPU execution provider");
        } else {
            tracing::debug!("Session is not initialized.");
        }
    }

    fn set_sess(&mut self, sess: Session);
    fn sess(&self) -> Option<&Session>;
}