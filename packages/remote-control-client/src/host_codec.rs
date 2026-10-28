//! Internal codec selection. Remote Control and resource ownership stay in the host.
use lingxi_webrtc_core::ReceivedRtp;
use lingxi_webrtc_media::{ports::DecodedAudioFrame, video_ports::RemoteVideoFrame, EncodedAudio};
use media_api::{MediaError, MediaErrorKind};

#[derive(Clone, Default)]
pub(crate) enum CodecConfig {
    #[default]
    Local,
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    Isolated(crate::isolated_codec::CodecLaunch),
}
#[derive(Clone)]
pub(crate) enum HostCodec {
    Local,
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    Isolated(crate::isolated_codec::CodecClient),
}
fn unavailable() -> MediaError {
    MediaError::new(
        MediaErrorKind::Unavailable,
        "media codec runtime is unavailable",
    )
}
impl CodecConfig {
    pub(crate) async fn open(&self, audio: bool, video: bool) -> Result<HostCodec, MediaError> {
        match self {
            Self::Local => {
                lingxi_webrtc_media::runtime::initialize_configured(video)
                    .map_err(|_| unavailable())?;
                Ok(HostCodec::Local)
            }
            #[cfg(not(any(target_os = "android", target_os = "ios")))]
            Self::Isolated(launch) => {
                crate::isolated_codec::CodecClient::spawn(launch.clone(), audio, video)
                    .await
                    .map(HostCodec::Isolated)
                    .map_err(|_| unavailable())
            }
        }
    }
}
impl HostCodec {
    pub(crate) fn isolated(&self) -> bool {
        match self {
            Self::Local => false,
            #[cfg(not(any(target_os = "android", target_os = "ios")))]
            Self::Isolated(_) => true,
        }
    }
    pub(crate) async fn shutdown(&self) -> Result<(), MediaError> {
        match self {
            Self::Local => Ok(()),
            #[cfg(not(any(target_os = "android", target_os = "ios")))]
            Self::Isolated(codec) => codec.shutdown().await.map_err(|_| {
                MediaError::new(
                    MediaErrorKind::NativeFailure,
                    "media codec cleanup could not be confirmed",
                )
            }),
        }
    }
    pub(crate) async fn push_audio(&self, packet: ReceivedRtp) -> Result<(), MediaError> {
        match self {
            Self::Local => {
                drop(packet);
                Err(unavailable())
            }
            #[cfg(not(any(target_os = "android", target_os = "ios")))]
            Self::Isolated(codec) => codec.push_audio(packet).await.map_err(|_| unavailable()),
        }
    }
    pub(crate) async fn poll_audio(&self) -> Result<Option<DecodedAudioFrame>, MediaError> {
        match self {
            Self::Local => Err(unavailable()),
            #[cfg(not(any(target_os = "android", target_os = "ios")))]
            Self::Isolated(codec) => codec.poll_audio().await.map_err(|_| unavailable()),
        }
    }
    pub(crate) async fn encode(
        &self,
        rate: u32,
        samples: &[f32],
    ) -> Result<Vec<EncodedAudio>, MediaError> {
        match self {
            Self::Local => Err(unavailable()),
            #[cfg(not(any(target_os = "android", target_os = "ios")))]
            Self::Isolated(codec) => codec
                .encode_audio(rate, samples)
                .await
                .map_err(|_| unavailable()),
        }
    }
    pub(crate) async fn reset(&self) -> Result<(), MediaError> {
        match self {
            Self::Local => Err(unavailable()),
            #[cfg(not(any(target_os = "android", target_os = "ios")))]
            Self::Isolated(codec) => codec.reset_audio().await.map_err(|_| unavailable()),
        }
    }
    pub(crate) async fn push_video(&self, packet: ReceivedRtp) -> Result<(), MediaError> {
        match self {
            Self::Local => {
                drop(packet);
                Err(unavailable())
            }
            #[cfg(not(any(target_os = "android", target_os = "ios")))]
            Self::Isolated(codec) => codec.push_video(packet).await.map_err(|_| unavailable()),
        }
    }
    pub(crate) async fn enable_video(&self, enabled: bool) -> Result<(), MediaError> {
        match self {
            Self::Local => {
                let _ = enabled;
                Err(unavailable())
            }
            #[cfg(not(any(target_os = "android", target_os = "ios")))]
            Self::Isolated(codec) => codec.enable_video(enabled).await.map_err(|_| unavailable()),
        }
    }
    pub(crate) async fn poll_video(&self) -> Result<Vec<RemoteVideoFrame>, MediaError> {
        match self {
            Self::Local => Err(unavailable()),
            #[cfg(not(any(target_os = "android", target_os = "ios")))]
            Self::Isolated(codec) => codec.poll_video().await.map_err(|_| unavailable()),
        }
    }
}
