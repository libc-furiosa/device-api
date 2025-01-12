use std::collections::{HashMap, HashSet};
use std::ops::Deref;

use crate::arch::Arch;
use crate::device::{CoreIdx, CoreStatus, Device, DeviceFile, DeviceMode};
use crate::error::DeviceResult;

#[derive(Copy, Clone)]
pub struct DeviceConfig {
    arch: Arch,
    mode: DeviceMode,
    count: u8,
}

impl DeviceConfig {
    pub fn warboy() -> WarboyConfigBuilder {
        let builder = DeviceConfig {
            arch: Arch::Warboy,
            mode: DeviceMode::Single,
            count: 1,
        };
        WarboyConfigBuilder(builder)
    }
}

impl Default for DeviceConfig {
    fn default() -> Self {
        DeviceConfig::warboy().fused().count(1)
    }
}
pub struct WarboyConfigBuilder(DeviceConfig);

impl WarboyConfigBuilder {
    pub fn multicore(mut self) -> Self {
        self.0.mode = DeviceMode::MultiCore;
        self
    }

    pub fn fused(mut self) -> Self {
        self.0.mode = DeviceMode::Fusion;
        self
    }

    pub fn count(mut self, count: u8) -> DeviceConfig {
        self.0.count = count;
        self.0
    }

    pub fn build(self) -> DeviceConfig {
        self.0
    }
}

pub(crate) struct DeviceWithStatus {
    pub device: Device,
    pub statuses: HashMap<CoreIdx, CoreStatus>,
}

impl Deref for DeviceWithStatus {
    type Target = Device;

    fn deref(&self) -> &Self::Target {
        &self.device
    }
}

pub(crate) async fn expand_status(devices: Vec<Device>) -> DeviceResult<Vec<DeviceWithStatus>> {
    let mut new_devices = Vec::with_capacity(devices.len());
    for device in devices.into_iter() {
        new_devices.push(DeviceWithStatus {
            statuses: device.get_status_all().await?,
            device,
        })
    }
    Ok(new_devices)
}

pub(crate) fn find_devices_in(
    config: &DeviceConfig,
    devices: &[DeviceWithStatus],
) -> DeviceResult<Vec<DeviceFile>> {
    let mut allocated: HashMap<u8, HashSet<u8>> = HashMap::with_capacity(devices.len());

    for device in devices {
        allocated.insert(
            device.device_index(),
            device
                .statuses
                .iter()
                .filter(|(_, status)| **status != CoreStatus::Available)
                .map(|(core, _)| *core)
                .collect(),
        );
    }

    let mut found: Vec<DeviceFile> = Vec::with_capacity(config.count.into());

    'outer: for _ in 0..config.count {
        for device in devices {
            if config.arch != device.arch() {
                continue;
            }
            // early exit for multicore
            if config.mode == DeviceMode::MultiCore
                && !allocated.get(&device.device_index()).unwrap().is_empty()
            {
                continue;
            }

            'inner: for dev_file in device
                .dev_files()
                .iter()
                .filter(|d| d.mode() == config.mode)
            {
                for idx in dev_file.indices() {
                    if allocated.get(&device.device_index()).unwrap().contains(idx) {
                        continue 'inner;
                    }
                }
                // this dev_file is suitable
                found.push(dev_file.clone());

                let used = allocated.get_mut(&device.device_index()).unwrap();
                used.extend(dev_file.indices());
                if dev_file.is_multicore() {
                    used.extend(device.cores());
                }
                continue 'outer;
            }
        }
        return Ok(vec![]);
    }

    Ok(found)
}

#[cfg(test)]
mod tests {
    use crate::list::list_devices_with;

    use super::*;

    #[tokio::test]
    async fn test_find_devices() -> DeviceResult<()> {
        // test directory contains 2 warboy NPUs
        let devices = list_devices_with("test_data/test-0/dev", "test_data/test-0/sys").await?;
        let devices_with_statuses = expand_status(devices).await?;

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
