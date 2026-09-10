use windows_sys::Win32::Devices::Display::{
    DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME, DISPLAYCONFIG_DEVICE_INFO_HEADER,
    DISPLAYCONFIG_MODE_INFO, DISPLAYCONFIG_PATH_INFO, DISPLAYCONFIG_SOURCE_DEVICE_NAME,
    DisplayConfigGetDeviceInfo, DisplayConfigSetDeviceInfo, GetDisplayConfigBufferSizes,
    QDC_ONLY_ACTIVE_PATHS, QueryDisplayConfig,
};
use windows_sys::Win32::Foundation::{ERROR_SUCCESS, LUID};

pub const SCALE_PERCENTS: [u32; 12] = [100, 125, 150, 175, 200, 225, 250, 300, 350, 400, 450, 500];

const GET_DPI_SCALE: i32 = -3;
const SET_DPI_SCALE: i32 = -4;

#[repr(C)]
#[derive(Clone, Copy)]
struct DpiScaleRequest {
    header: DISPLAYCONFIG_DEVICE_INFO_HEADER,
    minimum: i32,
    current: i32,
    maximum: i32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct DpiScaleAssignment {
    header: DISPLAYCONFIG_DEVICE_INFO_HEADER,
    relative: i32,
}

#[derive(Clone)]
pub struct DisplaySource {
    adapter: LUID,
    id: u32,
    pub device: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScaleRange {
    pub minimum: i32,
    pub current: i32,
    pub maximum: i32,
}

fn header(adapter: LUID, id: u32, kind: i32, size: usize) -> DISPLAYCONFIG_DEVICE_INFO_HEADER {
    DISPLAYCONFIG_DEVICE_INFO_HEADER {
        r#type: kind,
        size: size as u32,
        adapterId: adapter,
        id,
    }
}

pub fn source_for_device(device: &str) -> Result<DisplaySource, String> {
    let mut paths: u32 = 0;
    let mut modes: u32 = 0;
    let sizes =
        unsafe { GetDisplayConfigBufferSizes(QDC_ONLY_ACTIVE_PATHS, &mut paths, &mut modes) };
    if sizes != ERROR_SUCCESS {
        return Err(format!("GetDisplayConfigBufferSizes failed with {sizes}"));
    }
    let mut path_array: Vec<DISPLAYCONFIG_PATH_INFO> =
        vec![unsafe { std::mem::zeroed() }; paths as usize];
    let mut mode_array: Vec<DISPLAYCONFIG_MODE_INFO> =
        vec![unsafe { std::mem::zeroed() }; modes as usize];
    let query = unsafe {
        QueryDisplayConfig(
            QDC_ONLY_ACTIVE_PATHS,
            &mut paths,
            path_array.as_mut_ptr(),
            &mut modes,
            mode_array.as_mut_ptr(),
            std::ptr::null_mut(),
        )
    };
    if query != ERROR_SUCCESS {
        return Err(format!("QueryDisplayConfig failed with {query}"));
    }
    for path in path_array.iter().take(paths as usize) {
        let mut name = DISPLAYCONFIG_SOURCE_DEVICE_NAME {
            header: header(
                path.sourceInfo.adapterId,
                path.sourceInfo.id,
                DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME,
                std::mem::size_of::<DISPLAYCONFIG_SOURCE_DEVICE_NAME>(),
            ),
            viewGdiDeviceName: [0; 32],
        };
        let status = unsafe {
            DisplayConfigGetDeviceInfo(&mut name as *mut _ as *mut DISPLAYCONFIG_DEVICE_INFO_HEADER)
        };
        if status != 0 {
            continue;
        }
        let length = name
            .viewGdiDeviceName
            .iter()
            .position(|unit| *unit == 0)
            .unwrap_or(name.viewGdiDeviceName.len());
        if String::from_utf16_lossy(&name.viewGdiDeviceName[..length]) == device {
            return Ok(DisplaySource {
                adapter: path.sourceInfo.adapterId,
                id: path.sourceInfo.id,
                device: device.to_string(),
            });
        }
    }
    Err(format!("no active display source is named {device}"))
}

pub fn scale_range(source: &DisplaySource) -> Result<ScaleRange, String> {
    let mut request = DpiScaleRequest {
        header: header(
            source.adapter,
            source.id,
            GET_DPI_SCALE,
            std::mem::size_of::<DpiScaleRequest>(),
        ),
        minimum: 0,
        current: 0,
        maximum: 0,
    };
    let status = unsafe {
        DisplayConfigGetDeviceInfo(&mut request as *mut _ as *mut DISPLAYCONFIG_DEVICE_INFO_HEADER)
    };
    if status != 0 {
        return Err(format!(
            "reading the display scale of {} failed with {status}",
            source.device
        ));
    }
    Ok(ScaleRange {
        minimum: request.minimum,
        current: request.current,
        maximum: request.maximum,
    })
}

pub fn relative_for_percent(
    range: ScaleRange,
    current_percent: u32,
    wanted_percent: u32,
) -> Result<i32, String> {
    let index = |percent: u32| {
        SCALE_PERCENTS
            .iter()
            .position(|value| *value == percent)
            .map(|position| position as i32)
            .ok_or_else(|| format!("{percent}% is not a Windows display scale"))
    };
    let recommended = index(current_percent)? - range.current;
    let relative = index(wanted_percent)? - recommended;
    if relative < range.minimum || relative > range.maximum {
        return Err(format!(
            "{wanted_percent}% is outside the display scale range {}..={}",
            range.minimum, range.maximum
        ));
    }
    Ok(relative)
}

pub struct ScaleOverride {
    source: DisplaySource,
    restore: i32,
    applied: bool,
}

impl ScaleOverride {
    pub fn apply(source: &DisplaySource, range: ScaleRange, relative: i32) -> Result<Self, String> {
        let mut guard = Self {
            source: source.clone(),
            restore: range.current,
            applied: false,
        };
        assign(source, relative)?;
        guard.applied = true;
        Ok(guard)
    }

    pub fn set(&mut self, relative: i32) -> Result<(), String> {
        assign(&self.source, relative)?;
        self.applied = true;
        Ok(())
    }

    pub fn restore(&mut self) -> Result<(), String> {
        if !self.applied {
            return Ok(());
        }
        self.applied = false;
        assign(&self.source, self.restore)
    }
}

impl Drop for ScaleOverride {
    fn drop(&mut self) {
        let _ = self.restore();
    }
}

fn assign(source: &DisplaySource, relative: i32) -> Result<(), String> {
    let assignment = DpiScaleAssignment {
        header: header(
            source.adapter,
            source.id,
            SET_DPI_SCALE,
            std::mem::size_of::<DpiScaleAssignment>(),
        ),
        relative,
    };
    let status = unsafe {
        DisplayConfigSetDeviceInfo(
            &assignment as *const _ as *const DISPLAYCONFIG_DEVICE_INFO_HEADER,
        )
    };
    if status != 0 {
        return Err(format!(
            "setting the display scale of {} to relative {relative} failed with {status}",
            source.device
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_recommended_scale_of_one_hundred_percent_maps_the_qualified_modes() {
        let range = ScaleRange {
            minimum: 0,
            current: 0,
            maximum: 5,
        };
        assert_eq!(relative_for_percent(range, 100, 150), Ok(2));
        assert_eq!(relative_for_percent(range, 100, 200), Ok(4));
        assert_eq!(relative_for_percent(range, 100, 100), Ok(0));
    }

    #[test]
    fn the_recommended_index_is_derived_from_the_observed_scale() {
        let range = ScaleRange {
            minimum: -2,
            current: 1,
            maximum: 4,
        };
        assert_eq!(relative_for_percent(range, 150, 150), Ok(1));
        assert_eq!(relative_for_percent(range, 150, 200), Ok(3));
        assert_eq!(relative_for_percent(range, 150, 100), Ok(-1));
    }

    #[test]
    fn a_mode_outside_the_reported_range_is_refused() {
        let range = ScaleRange {
            minimum: 0,
            current: 0,
            maximum: 1,
        };
        assert!(relative_for_percent(range, 100, 200).is_err());
        assert!(relative_for_percent(range, 100, 160).is_err());
    }
}
