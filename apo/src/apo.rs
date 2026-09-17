//! Die eigentliche COM-Klasse, die Windows als Audio-Filter lädt.
//!
//! Wichtig: `APOProcess` läuft im Echtzeit-Thread des Audiodienstes. Dort nichts anlegen,
//! nichts sperren, nicht blockieren. Ein Absturz hier legt den Ton von Windows lahm.

use std::cell::UnsafeCell;
use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::OnceLock;

use windows::Win32::Foundation::{CLASS_E_CLASSNOTAVAILABLE, CLASS_E_NOAGGREGATION, E_POINTER, HANDLE, S_FALSE};
use windows::Win32::Media::Audio::Apo::{
    APO_CONNECTION_DESCRIPTOR, APO_CONNECTION_PROPERTY, APO_FLAG_DEFAULT, APO_REG_PROPERTIES, BUFFER_VALID,
    IAudioMediaType, IAudioProcessingObject, IAudioProcessingObject_Impl, IAudioProcessingObjectConfiguration,
    IAudioProcessingObjectConfiguration_Impl, IAudioProcessingObjectRT, IAudioProcessingObjectRT_Impl, IAudioSystemEffects_Impl,
    IAudioSystemEffects2, IAudioSystemEffects2_Impl, UNCOMPRESSEDAUDIOFORMAT,
};
use windows::Win32::System::Com::{CoTaskMemAlloc, IClassFactory, IClassFactory_Impl};
use windows::core::{BOOL, GUID, HRESULT, IUnknown, Interface, Ref, Result, implement};

use crate::dsp::Chain;
use crate::shared::{Feedback, Mapping};

pub const APO_CLSID: GUID = GUID::from_u128(0x6b2f3c1e_8d4a_4f5b_9c2e_7a1d0e5f3b40);
pub const APO_CLSID_STRING: &str = "{6B2F3C1E-8D4A-4F5B-9C2E-7A1D0E5F3B40}";

/// KSDATAFORMAT_SUBTYPE_IEEE_FLOAT
const FLOAT_FORMAT: GUID = GUID::from_u128(0x00000003_0000_0010_8000_00aa00389b71);

/// Gemeinsamer Speicher, einmal pro Prozess geöffnet und von allen Filter-Instanzen geteilt.
/// Absichtlich nie freigegeben: er lebt so lange wie der Audiodienst.
static SHARED: OnceLock<Option<Mapping>> = OnceLock::new();

/// Öffnen darf nur außerhalb des Echtzeit-Threads passieren (LockForProcess).
fn open_shared() -> bool {
    SHARED.get_or_init(Mapping::create).is_some()
}

/// Im Echtzeit-Thread: nur nachsehen, nie selbst öffnen.
fn shared_now() -> Option<&'static Mapping> {
    SHARED.get().and_then(Option::as_ref)
}

#[implement(IAudioProcessingObject, IAudioProcessingObjectRT, IAudioProcessingObjectConfiguration, IAudioSystemEffects2)]
struct LaermampelApo {
    channels: AtomicU32,
    bytes_per_sample: AtomicU32,
    is_float: AtomicBool,
    /// Gate, Comp., Fader, Limiter. Angelegt in LockForProcess, benutzt nur im Echtzeit-Thread.
    chain: UnsafeCell<Option<Chain>>,
}

impl LaermampelApo {
    fn new() -> Self {
        Self {
            channels: AtomicU32::new(1),
            bytes_per_sample: AtomicU32::new(4),
            is_float: AtomicBool::new(false),
            chain: UnsafeCell::new(None),
        }
    }
}

impl IAudioProcessingObject_Impl for LaermampelApo_Impl {
    fn Reset(&self) -> Result<()> {
        Ok(())
    }

    fn GetLatency(&self) -> Result<i64> {
        Ok(0)
    }

    fn GetRegistrationProperties(&self) -> Result<*mut APO_REG_PROPERTIES> {
        unsafe {
            let properties = CoTaskMemAlloc(size_of::<APO_REG_PROPERTIES>()) as *mut APO_REG_PROPERTIES;
            if properties.is_null() {
                return Err(E_POINTER.into());
            }
            properties.write(registration_properties());
            Ok(properties)
        }
    }

    fn Initialize(&self, _cbdatasize: u32, _pbydata: *const u8) -> Result<()> {
        Ok(())
    }

    // Jedes Format annehmen und unverändert durchreichen. Bearbeitet wird nur Float,
    // alles andere geht unangetastet durch, damit das Mikrofon auf keinen Fall ausfällt.
    fn IsInputFormatSupported(&self, _opposite: Ref<IAudioMediaType>, requested: Ref<IAudioMediaType>) -> Result<IAudioMediaType> {
        Ok(requested.ok()?.clone())
    }

    fn IsOutputFormatSupported(&self, _opposite: Ref<IAudioMediaType>, requested: Ref<IAudioMediaType>) -> Result<IAudioMediaType> {
        Ok(requested.ok()?.clone())
    }

    fn GetInputChannelCount(&self) -> Result<u32> {
        Ok(self.channels.load(Ordering::Relaxed))
    }
}

impl IAudioProcessingObjectConfiguration_Impl for LaermampelApo_Impl {
    fn LockForProcess(
        &self,
        num_inputs: u32,
        inputs: *const *const APO_CONNECTION_DESCRIPTOR,
        _num_outputs: u32,
        _outputs: *const *const APO_CONNECTION_DESCRIPTOR,
    ) -> Result<()> {
        if num_inputs < 1 || inputs.is_null() {
            return Err(E_POINTER.into());
        }
        unsafe {
            let input = &**inputs;
            if let Some(format) = input.pFormat.as_ref() {
                let mut info = UNCOMPRESSEDAUDIOFORMAT::default();
                if format.GetUncompressedAudioFormat(&mut info).is_ok() {
                    self.channels.store(info.dwSamplesPerFrame.max(1), Ordering::Relaxed);
                    self.bytes_per_sample.store(info.dwBytesPerSampleContainer.max(1), Ordering::Relaxed);
                    let float = info.guidFormatType == FLOAT_FORMAT && info.dwBytesPerSampleContainer == 4;
                    self.is_float.store(float, Ordering::Relaxed);
                    // Windows ruft LockForProcess nie gleichzeitig mit APOProcess auf.
                    *self.chain.get() = Some(Chain::new(info.fFramesPerSecond.max(8000.0)));
                }
            }
        }
        open_shared();
        Ok(())
    }

    fn UnlockForProcess(&self) -> Result<()> {
        Ok(())
    }
}

impl IAudioProcessingObjectRT_Impl for LaermampelApo_Impl {
    fn APOProcess(
        &self,
        num_inputs: u32,
        inputs: *const *const APO_CONNECTION_PROPERTY,
        num_outputs: u32,
        outputs: *mut *mut APO_CONNECTION_PROPERTY,
    ) {
        if num_inputs < 1 || num_outputs < 1 || inputs.is_null() || outputs.is_null() {
            return;
        }
        unsafe {
            let input = &**inputs;
            let output = &mut **outputs;
            let channels = self.channels.load(Ordering::Relaxed) as usize;
            let frames = input.u32ValidFrameCount as usize;
            let bytes = frames * channels * self.bytes_per_sample.load(Ordering::Relaxed) as usize;

            if input.pBuffer != output.pBuffer && bytes > 0 {
                std::ptr::copy_nonoverlapping(input.pBuffer as *const u8, output.pBuffer as *mut u8, bytes);
            }
            output.u32ValidFrameCount = input.u32ValidFrameCount;
            output.u32BufferFlags = input.u32BufferFlags;

            let Some(params) = shared_now().map(Mapping::params) else { return };
            params.beat();

            if !self.is_float.load(Ordering::Relaxed) || input.u32BufferFlags != BUFFER_VALID || frames == 0 {
                return;
            }

            let Some(chain) = (*self.chain.get()).as_mut() else { return };
            let samples = std::slice::from_raw_parts_mut(output.pBuffer as *mut f32, frames * channels);

            // Pegel vor der Bearbeitung, für die Ampel.
            let power = samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32;
            let input_level_db = 10.0 * power.max(1e-12).log10();

            chain.set(params.settings());
            for frame in samples.chunks_exact_mut(channels) {
                chain.process_frame(frame);
            }

            params.set_feedback(&Feedback {
                input_level_db,
                out_level_db: chain.out_level_db(),
                out_peak_db: chain.out_peak_db(),
                gate_open: chain.gate_open(),
                gate_level_db: chain.gate_level_db(),
                comp_gain_db: chain.comp_gain_db(),
            });
        }
    }

    fn CalcInputFrames(&self, output_frames: u32) -> u32 {
        output_frames
    }

    fn CalcOutputFrames(&self, input_frames: u32) -> u32 {
        input_frames
    }
}

impl IAudioSystemEffects_Impl for LaermampelApo_Impl {}

impl IAudioSystemEffects2_Impl for LaermampelApo_Impl {
    fn GetEffectsList(&self, effect_ids: *mut *mut GUID, count: *mut u32, _event: HANDLE) -> Result<()> {
        unsafe {
            if !effect_ids.is_null() {
                *effect_ids = std::ptr::null_mut();
            }
            if !count.is_null() {
                *count = 0;
            }
        }
        Ok(())
    }
}

fn wide_fixed(text: &str) -> [u16; 256] {
    let mut buffer = [0u16; 256];
    for (slot, unit) in buffer.iter_mut().take(255).zip(text.encode_utf16()) {
        *slot = unit;
    }
    buffer
}

fn registration_properties() -> APO_REG_PROPERTIES {
    APO_REG_PROPERTIES {
        clsid: APO_CLSID,
        Flags: APO_FLAG_DEFAULT,
        szFriendlyName: wide_fixed("Lärmampel"),
        szCopyrightInfo: wide_fixed("LucyWolf"),
        u32MajorVersion: 1,
        u32MinorVersion: 0,
        u32MinInputConnections: 1,
        u32MaxInputConnections: 1,
        u32MinOutputConnections: 1,
        u32MaxOutputConnections: 1,
        u32MaxInstances: u32::MAX,
        u32NumAPOInterfaces: 1,
        iidAPOInterfaceList: [IAudioProcessingObject::IID],
    }
}

#[implement(IClassFactory)]
struct Factory;

impl IClassFactory_Impl for Factory_Impl {
    fn CreateInstance(&self, outer: Ref<IUnknown>, iid: *const GUID, object: *mut *mut c_void) -> Result<()> {
        if object.is_null() {
            return Err(E_POINTER.into());
        }
        unsafe { *object = std::ptr::null_mut() };
        if !outer.is_null() {
            return Err(CLASS_E_NOAGGREGATION.into());
        }
        let apo: IUnknown = LaermampelApo::new().into();
        unsafe { apo.query(iid, object).ok() }
    }

    fn LockServer(&self, _lock: BOOL) -> Result<()> {
        Ok(())
    }
}

/// Einstiegspunkt für COM: Windows fragt hier nach der Fabrik für unseren Filter.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn DllGetClassObject(clsid: *const GUID, iid: *const GUID, object: *mut *mut c_void) -> HRESULT {
    if clsid.is_null() || iid.is_null() || object.is_null() {
        return E_POINTER;
    }
    unsafe {
        *object = std::ptr::null_mut();
        if *clsid != APO_CLSID {
            return CLASS_E_CLASSNOTAVAILABLE;
        }
        let factory: IClassFactory = Factory.into();
        factory.query(iid, object)
    }
}

/// Nie entladen: der Audiodienst hält uns ohnehin, und so bleibt der gemeinsame Speicher gültig.
#[unsafe(no_mangle)]
pub extern "system" fn DllCanUnloadNow() -> HRESULT {
    S_FALSE
}
