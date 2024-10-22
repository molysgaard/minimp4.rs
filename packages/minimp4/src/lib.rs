#[cfg(feature = "aac")]
pub mod enc;
mod writer;

use std::{
    convert::TryInto,
    ffi::CString,
    io::{Seek, SeekFrom, Write},
    mem::size_of,
    os::raw::c_void,
    ptr::null_mut,
    slice::from_raw_parts,
};

#[cfg(feature = "aac")]
use enc::{BitRate, EncoderParams};
use libc::malloc;
use minimp4_sys::{
    mp4_h26x_write_init, mp4_h26x_writer_t, MP4E_close, MP4E_mux_t, MP4E_open, MP4E_set_text_comment,
    MP4E_STATUS_BAD_ARGUMENTS,
};
#[cfg(feature = "aac")]
use writer::write_mp4_with_audio;
use writer::{write_mp4, write_mp4_frame_with_duration};

pub struct Mp4Muxer<W> {
    writer: W,
    muxer: *mut MP4E_mux_t,
    muxer_writer: *mut mp4_h26x_writer_t,
    str_buffer: Vec<CString>,
    #[cfg(feature = "aac")]
    encoder_params: Option<EncoderParams>,
}

#[derive(Debug)]
#[repr(i32)]
pub enum Minimp4ReturnCode {
    Ok = minimp4_sys::MP4E_STATUS_OK as i32,
    Err(Minimp4Error),
}

impl TryFrom<i32> for Minimp4ReturnCode {
    type Error = ();
    fn try_from(value: i32) -> Result<Self, Self::Error> {
        Ok(if value == minimp4_sys::MP4E_STATUS_OK as i32 {
            Self::Ok
        } else {
            Self::Err(Minimp4Error::try_from(value)?)
        })
    }
}

#[derive(Debug)]
#[repr(i32)]
pub enum Minimp4Error {
    BadArguments = minimp4_sys::MP4E_STATUS_BAD_ARGUMENTS,
    NoMemory = minimp4_sys::MP4E_STATUS_NO_MEMORY,
    FileWriteError = minimp4_sys::MP4E_STATUS_FILE_WRITE_ERROR,
    OnlyOneDsiAllowed = minimp4_sys::MP4E_STATUS_ONLY_ONE_DSI_ALLOWED,
}

impl TryFrom<i32> for Minimp4Error {
    type Error = ();
    fn try_from(value: i32) -> Result<Self, Self::Error> {
        match value {
            minimp4_sys::MP4E_STATUS_BAD_ARGUMENTS => Ok(Self::BadArguments),
            minimp4_sys::MP4E_STATUS_NO_MEMORY => Ok(Self::NoMemory),
            minimp4_sys::MP4E_STATUS_FILE_WRITE_ERROR => Ok(Self::FileWriteError),
            minimp4_sys::MP4E_STATUS_ONLY_ONE_DSI_ALLOWED => Ok(Self::OnlyOneDsiAllowed),
            _ => Err(()),
        }
    }
}

pub type Minimp4Result<T> = Result<T, Minimp4Error>;

impl From<Minimp4ReturnCode> for Minimp4Result<()> {
    fn from(value: Minimp4ReturnCode) -> Self {
        match value {
            Minimp4ReturnCode::Err(e) => Err(e),
            Minimp4ReturnCode::Ok => Ok(()),
        }
    }
}

impl<W: Write + Seek> Mp4Muxer<W> {
    pub fn new(writer: W) -> Self {
        unsafe {
            Self {
                writer,
                muxer: null_mut(),
                muxer_writer: malloc(size_of::<mp4_h26x_writer_t>()) as *mut mp4_h26x_writer_t,
                str_buffer: Vec::new(),
                #[cfg(feature = "aac")]
                encoder_params: None,
            }
        }
    }

    pub fn init_video(&mut self, width: i32, height: i32, is_hevc: bool, track_name: &str) {
        self.str_buffer.push(CString::new(track_name).unwrap());
        unsafe {
            if self.muxer.is_null() {
                let self_ptr = self as *mut Self as *mut c_void;
                self.muxer = MP4E_open(0, 0, self_ptr, Some(Self::write));
            }
            mp4_h26x_write_init(
                self.muxer_writer,
                self.muxer,
                width,
                height,
                if is_hevc { 1 } else { 0 },
            );
        }
    }

    #[cfg(feature = "aac")]
    pub fn init_audio(&mut self, bit_rate: u32, sample_rate: u32, channel_count: u32) {
        self.encoder_params = Some(EncoderParams {
            bit_rate: BitRate::Cbr(bit_rate),
            sample_rate,
            channel_count,
        });
    }

    pub fn write_video(&self, data: &[u8]) -> Minimp4Result<()> {
        self.write_video_with_fps(data, 60)
    }

    #[cfg(feature = "aac")]
    pub fn write_video_with_audio(&self, data: &[u8], fps: u32, pcm: &[u8]) {
        assert!(self.encoder_params.is_some());
        let mp4wr = unsafe { self.muxer_writer.as_mut().unwrap() };
        let fps = fps.try_into().unwrap();
        let encoder_params = self.encoder_params.unwrap();
        write_mp4_with_audio(mp4wr, fps, data, pcm, encoder_params)
    }

    pub fn write_video_with_fps(&self, data: &[u8], fps: u32) -> Minimp4Result<()> {
        let mp4wr = unsafe { self.muxer_writer.as_mut().unwrap() };
        let fps = fps.try_into().unwrap();
        write_mp4(mp4wr, fps, data)
    }

    pub fn write_frame_with_duration(&self, data: &[u8], duration_90KHz: u32) -> Minimp4Result<()> {
        let mp4wr = unsafe { self.muxer_writer.as_mut().unwrap() };
        write_mp4_frame_with_duration(mp4wr, duration_90KHz, data)
    }

    pub fn write_comment(&mut self, comment: &str) {
        self.str_buffer.push(CString::new(comment).unwrap());
        unsafe {
            MP4E_set_text_comment(self.muxer, self.str_buffer.last().unwrap().as_ptr());
        }
    }
    pub fn close(&self) -> &W {
        unsafe {
            MP4E_close(self.muxer);
        }
        &self.writer
    }

    pub fn write_data(&mut self, offset: i64, buf: &[u8]) -> usize {
        self.writer.seek(SeekFrom::Start(offset as u64)).unwrap();
        self.writer.write(buf).unwrap_or(0)
    }

    extern "C" fn write(offset: i64, buffer: *const c_void, size: usize, token: *mut c_void) -> i32 {
        let p_self = token as *mut Self;
        unsafe {
            let buf = from_raw_parts(buffer as *const u8, size);
            ((*p_self).write_data(offset, buf) != size) as i32
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;

    #[test]
    fn test_muxer() {
        let mut muxer = Mp4Muxer::new(Cursor::new(Vec::new()));
        muxer.init_video(1280, 720, false, "test");
        muxer.write_video(&[0; 100]);
        muxer.write_comment("test comment");
        muxer.close();
        assert_eq!(muxer.writer.into_inner().len(), 257);
    }

    #[test]
    #[cfg(feature = "aac")]
    #[ignore = "not complete yet, some platform cannot link fdk-aac"]
    fn test_mux_h264_audio() {
        use std::{fs::write, path::Path};
        let mut buffer = Cursor::new(vec![]);
        let mut mp4muxer = Mp4Muxer::new(&mut buffer);
        let h264 = include_bytes!("./fixtures/input.264");
        let pcm = include_bytes!("./fixtures/input.pcm");
        mp4muxer.init_video(1280, 720, false, "h264 stream");
        mp4muxer.init_audio(128000, 44100, 2);
        mp4muxer.write_video_with_audio(h264, 25, pcm);
        mp4muxer.write_comment("test comment");
        mp4muxer.close();
        // write with audio has not stable output, need to be check later
        write(Path::new("./src/fixtures/h264_output.tmp"), buffer.into_inner()).unwrap();
    }

    #[test]
    fn test_mux_h264() {
        let mut buffer = Cursor::new(vec![]);
        let mut mp4muxer = Mp4Muxer::new(&mut buffer);
        let h264 = include_bytes!("./fixtures/input.264");
        mp4muxer.init_video(1280, 720, false, "h264 stream");
        mp4muxer.write_video_with_fps(h264, 25);
        mp4muxer.write_comment("test comment");
        mp4muxer.close();
        let buffer = buffer.into_inner();
        assert_eq!(buffer, include_bytes!("./fixtures/h264_output.mp4"));
    }

    #[test]
    fn test_mux_h265() {
        let mut buffer = Cursor::new(vec![]);
        let mut mp4muxer = Mp4Muxer::new(&mut buffer);
        let h265 = include_bytes!("./fixtures/input.265");
        mp4muxer.init_video(1280, 720, true, "h265 stream");
        mp4muxer.write_video_with_fps(h265, 25);
        mp4muxer.write_comment("test comment");
        mp4muxer.close();
        let buffer = buffer.into_inner();
        assert_eq!(buffer, include_bytes!("./fixtures/h265_output.mp4"));
    }
}
