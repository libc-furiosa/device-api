use std::collections::HashMap;
use std::fs::OpenOptions;
use std::io;
use std::path::Path;

use crate::device::{CoreIdx, CoreStatus, DeviceInfo};
use crate::find::DeviceWithStatus;
use crate::list::{collect_devices, filter_dev_files, DevFile, MGMT_FILES};
use crate::status::DeviceStatus;
use crate::sysfs::npu_mgmt;
use crate::sysfs::npu_mgmt::PLATFORM_TYPE;
use crate::{find_devices_in, Device, DeviceConfig, DeviceFile, DeviceResult};

pub fn list_devices() -> DeviceResult<Vec<Device>> {
    list_devices_with("/dev", "/sys")
}

pub fn find_devices(config: &DeviceConfig) -> DeviceResult<Vec<DeviceFile>> {
    let devices = expand_status(list_devices()?)?;
    find_devices_in(config, &devices)
}

/// Allow to specify arbitrary sysfs, devfs paths for unit testing
pub(crate) fn list_devices_with(devfs: &str, sysfs: &str) -> DeviceResult<Vec<Device>> {
    let npu_dev_files = filter_dev_files(list_devfs(devfs)?)?;

    let mut devices: Vec<Device> = Vec::with_capacity(npu_dev_files.len());

    for (idx, paths) in npu_dev_files {
        if is_furiosa_device(idx, sysfs) {
            let mgmt_files = read_mgmt_files(sysfs, idx)?;
            let device_info = DeviceInfo::try_from(mgmt_files)?;
            let device = collect_devices(idx, device_info, paths)?;
            devices.push(device);
        }
    }

    devices.sort();
    Ok(devices)
}

fn list_devfs<P: AsRef<Path>>(devfs: P) -> io::Result<Vec<DevFile>> {
    let mut dev_files = Vec::new();

    for entry in std::fs::read_dir(devfs)? {
        let file = entry?;
        dev_files.push(DevFile {
            path: file.path(),
            file_type: file.file_type()?,
        });
    }

    Ok(dev_files)
}

fn is_furiosa_device(idx: u8, sysfs: &str) -> bool {
    std::fs::read_to_string(npu_mgmt::path(sysfs, PLATFORM_TYPE, idx))
        .ok()
        .filter(|c| npu_mgmt::is_furiosa_platform(c))
        .is_some()
}

fn read_mgmt_files(sysfs: &str, idx: u8) -> io::Result<HashMap<&'static str, String>> {
    let mut mgmt_files: HashMap<&'static str, String> = HashMap::new();
    for mgmt_file in MGMT_FILES {
        let path = npu_mgmt::path(sysfs, mgmt_file, idx);
        let contents = std::fs::read_to_string(&path).map(|s| s.trim().to_string())?;
        if mgmt_files.insert(mgmt_file, contents).is_some() {
            unreachable!("duplicate {} file at {}", mgmt_file, path.display());
        }
    }
    Ok(mgmt_files)
}

pub(crate) fn expand_status(devices: Vec<Device>) -> DeviceResult<Vec<DeviceWithStatus>> {
    let mut new_devices = Vec::with_capacity(devices.len());
    for device in devices.into_iter() {
        new_devices.push(DeviceWithStatus {
            statuses: get_status_all(&device)?,
            device,
        })
    }
    Ok(new_devices)
}

pub fn get_device_status<P>(path: P) -> DeviceResult<DeviceStatus>
where
    P: AsRef<Path>,
{
    let res = OpenOptions::new().read(true).write(true).open(path);

    match res {
        Ok(_) => Ok(DeviceStatus::Available),
        Err(err) => {
            if err.raw_os_error().unwrap_or(0) == 16 {
                Ok(DeviceStatus::Occupied)
            } else {
                Err(err.into())
            }
        }
    }
}

pub fn get_status_all(device: &Device) -> DeviceResult<HashMap<CoreIdx, CoreStatus>> {
    let mut status_map = device.new_status_map();

    for file in &device.dev_files {
        if get_device_status(&file.path)? == DeviceStatus::Occupied {
            for core in file.indices.iter().chain(
                file.is_multicore()
                    .then(|| device.cores.iter())
                    .into_iter()
                    .flatten(),
            ) {
                status_map.insert(*core, CoreStatus::Occupied(file.to_string()));
            }
        }
    }
    Ok(status_map)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_find_devices() -> DeviceResult<()> {
        // test directory contains 2 warboy NPUs
        let devices = list_devices_with("test_data/test-0/dev", "test_data/test-0/sys")?;
        let devices_with_statuses = expand_status(devices)?;

        // try lookup 4 different single cores
        let config = DeviceConfig::warboy().count(4);
        let found = find_devices_in(&config, &devices_with_statuses)?;
        assert_eq!(found.len(), 4);
        assert_eq!(found[0].filename(), "npu0pe0");
        assert_eq!(found[1].filename(), "npu0pe1");
        assert_eq!(found[2].filename(), "npu1pe0");
        assert_eq!(found[3].filename(), "npu1pe1");

        // looking for 5 different cores should fail
        let config = DeviceConfig::warboy().count(5);
        let found = find_devices_in(&config, &devices_with_statuses)?;
        assert_eq!(found, vec![]);

        // try lookup 2 different fused cores
        let config = DeviceConfig::warboy().fused().count(2);
        let found = find_devices_in(&config, &devices_with_statuses)?;
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].filename(), "npu0pe0-1");
        assert_eq!(found[1].filename(), "npu1pe0-1");

        // looking for 3 different fused cores should fail
        let config = DeviceConfig::warboy().fused().count(3);
        let found = find_devices_in(&config, &devices_with_statuses)?;
        assert_eq!(found, vec![]);

        Ok(())
    }
}
