use std::collections::BTreeMap;
use std::io::Cursor;

use kira::sound::static_sound::{StaticSoundData, StaticSoundHandle};
use kira::sound::PlaybackState;
use kira::{AudioManager, AudioManagerSettings, DefaultBackend, Tween};
use pal_asset::{LoadedAsset, ResourceManager};

#[derive(Clone, Debug)]
pub struct AudioConfig {
    pub enabled: bool,
}

impl Default for AudioConfig {
    fn default() -> Self {
        Self { enabled: true }
    }
}

pub struct AudioSystem {
    enabled: bool,
    manager: Option<AudioManager<DefaultBackend>>,
    groups: BTreeMap<PalSoundGroup, Vec<AudioSlot>>,
    primary_volume: PalVolume,
    group_volumes: BTreeMap<PalSoundGroup, PalVolume>,
}

impl AudioSystem {
    pub fn new(config: AudioConfig) -> anyhow::Result<Self> {
        let manager = if config.enabled {
            Some(AudioManager::<DefaultBackend>::new(
                AudioManagerSettings::default(),
            )?)
        } else {
            None
        };
        let mut groups = BTreeMap::new();
        let mut group_volumes = BTreeMap::new();
        for group in PalSoundGroup::ALL {
            groups.insert(
                group,
                (0..group.slot_count())
                    .map(|_| AudioSlot::default())
                    .collect(),
            );
            group_volumes.insert(group, PalVolume::MAX);
        }
        Ok(Self {
            enabled: config.enabled,
            manager,
            groups,
            primary_volume: PalVolume::MAX,
            group_volumes,
        })
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    pub fn load_static_from_resource(
        &mut self,
        resource_manager: &mut ResourceManager,
        name: &str,
        group: PalSoundGroup,
    ) -> anyhow::Result<AudioHandle> {
        let asset = resource_manager.open(name)?;
        self.load_static_asset(asset, group)
    }

    pub fn load_static_asset(
        &mut self,
        asset: LoadedAsset,
        group: PalSoundGroup,
    ) -> anyhow::Result<AudioHandle> {
        let slot_index = self.find_free_slot(group)?;
        let data = StaticSoundData::from_cursor(Cursor::new(asset.bytes))?;
        let slot = self.slot_mut(AudioHandle::new(group, slot_index))?;
        slot.name = Some(asset.name);
        slot.data = Some(data);
        slot.handle = None;
        slot.looping = false;
        slot.volume = PalVolume::MAX;
        slot.start_ms = 0;
        slot.end_ms = 0;
        slot.pan = 0;
        slot.frequency = 0;
        Ok(AudioHandle::new(group, slot_index))
    }

    pub fn copy_sound(
        &mut self,
        source: AudioHandle,
        target_group: PalSoundGroup,
    ) -> anyhow::Result<AudioHandle> {
        let (data, name) = {
            let source_slot = self.slot(source)?;
            let data = source_slot
                .data
                .as_ref()
                .ok_or_else(|| {
                    anyhow::anyhow!("audio handle {:?} has no loaded sound data", source)
                })?
                .clone();
            (data, source_slot.name.clone())
        };
        let slot_index = self.find_free_slot(target_group)?;
        let slot = self.slot_mut(AudioHandle::new(target_group, slot_index))?;
        slot.name = name;
        slot.data = Some(data);
        slot.handle = None;
        slot.looping = false;
        slot.volume = PalVolume::MAX;
        slot.start_ms = 0;
        slot.end_ms = 0;
        slot.pan = 0;
        slot.frequency = 0;
        Ok(AudioHandle::new(target_group, slot_index))
    }

    pub fn play(&mut self, handle: AudioHandle, looping: bool) -> anyhow::Result<()> {
        if !self.enabled {
            return Ok(());
        }
        let data = {
            let slot = self.slot(handle)?;
            let data = slot
                .data
                .as_ref()
                .ok_or_else(|| {
                    anyhow::anyhow!("audio handle {:?} has no loaded sound data", handle)
                })?
                .clone();
            if looping {
                data.loop_region(0.0..)
            } else {
                data
            }
        };
        let effective = self.effective_volume(handle)?;
        let decibels = effective.to_decibels() as f32;
        log::debug!(
            "[trace-audio] play handle={handle:?} looping={looping} effective_raw={} db={decibels:.2}",
            effective.raw()
        );
        let manager = self
            .manager
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("audio backend is disabled"))?;
        let mut static_handle = manager.play(data)?;
        static_handle.set_volume(decibels, Tween::default());
        let slot = self.slot_mut(handle)?;
        slot.handle = Some(static_handle);
        slot.looping = looping;
        Ok(())
    }

    pub fn stop(&mut self, handle: AudioHandle) -> anyhow::Result<()> {
        let slot = self.slot_mut(handle)?;
        if let Some(sound) = slot.handle.as_mut() {
            sound.stop(Tween::default());
        }
        slot.handle = None;
        Ok(())
    }

    pub fn pause(&mut self, handle: AudioHandle) -> anyhow::Result<()> {
        let slot = self.slot_mut(handle)?;
        if let Some(sound) = slot.handle.as_mut() {
            sound.pause(Tween::default());
        }
        Ok(())
    }

    pub fn resume(&mut self, handle: AudioHandle) -> anyhow::Result<()> {
        let slot = self.slot_mut(handle)?;
        if let Some(sound) = slot.handle.as_mut() {
            sound.resume(Tween::default());
        }
        Ok(())
    }

    pub fn release(&mut self, handle: AudioHandle) -> anyhow::Result<()> {
        self.stop(handle)?;
        let slot = self.slot_mut(handle)?;
        *slot = AudioSlot::default();
        Ok(())
    }

    pub fn release_group(&mut self, group: PalSoundGroup) -> anyhow::Result<()> {
        let len = self.groups.get(&group).map_or(0, Vec::len);
        for index in 0..len {
            self.release(AudioHandle::new(group, index))?;
        }
        Ok(())
    }

    pub fn set_primary_volume(&mut self, volume: PalVolume) -> anyhow::Result<()> {
        self.primary_volume = volume.clamped();
        log::debug!(
            "[trace-audio] set_primary_volume raw={}",
            self.primary_volume.raw()
        );
        self.apply_all_volumes()
    }

    pub fn primary_volume(&self) -> PalVolume {
        self.primary_volume
    }

    pub fn set_group_volume(
        &mut self,
        group: PalSoundGroup,
        volume: PalVolume,
    ) -> anyhow::Result<()> {
        self.group_volumes.insert(group, volume.clamped());
        log::debug!(
            "[trace-audio] set_group_volume group={group:?} raw={}",
            volume.clamped().raw()
        );
        let len = self.groups.get(&group).map_or(0, Vec::len);
        for index in 0..len {
            self.apply_slot_volume(AudioHandle::new(group, index))?;
        }
        Ok(())
    }

    pub fn group_volume(&self, group: PalSoundGroup) -> PalVolume {
        *self.group_volumes.get(&group).unwrap_or(&PalVolume::MAX)
    }

    pub fn set_channel_volume(
        &mut self,
        handle: AudioHandle,
        volume: PalVolume,
    ) -> anyhow::Result<()> {
        self.slot_mut(handle)?.volume = volume.clamped();
        log::debug!(
            "[trace-audio] set_channel_volume handle={handle:?} raw={}",
            volume.clamped().raw()
        );
        self.apply_slot_volume(handle)
    }

    pub fn channel_volume(&self, handle: AudioHandle) -> anyhow::Result<PalVolume> {
        Ok(self.slot(handle)?.volume)
    }

    pub fn set_start_end(
        &mut self,
        handle: AudioHandle,
        start_ms: i32,
        end_ms: i32,
    ) -> anyhow::Result<()> {
        let slot = self.slot_mut(handle)?;
        ensure_loaded(slot, handle)?;
        slot.start_ms = start_ms;
        slot.end_ms = end_ms;
        Ok(())
    }

    pub fn start_end(&self, handle: AudioHandle) -> anyhow::Result<(i32, i32)> {
        let slot = self.slot(handle)?;
        ensure_loaded(slot, handle)?;
        Ok((slot.start_ms, slot.end_ms))
    }

    pub fn set_channel_pan(&mut self, handle: AudioHandle, pan: i32) -> anyhow::Result<()> {
        let slot = self.slot_mut(handle)?;
        ensure_loaded(slot, handle)?;
        slot.pan = pan;
        Ok(())
    }

    pub fn channel_pan(&self, handle: AudioHandle) -> anyhow::Result<i32> {
        let slot = self.slot(handle)?;
        ensure_loaded(slot, handle)?;
        Ok(slot.pan)
    }

    pub fn set_channel_frequency(
        &mut self,
        handle: AudioHandle,
        frequency: i32,
    ) -> anyhow::Result<()> {
        let slot = self.slot_mut(handle)?;
        ensure_loaded(slot, handle)?;
        slot.frequency = frequency;
        Ok(())
    }

    pub fn channel_frequency(&self, handle: AudioHandle) -> anyhow::Result<i32> {
        let slot = self.slot(handle)?;
        ensure_loaded(slot, handle)?;
        Ok(slot.frequency)
    }

    pub fn sound_param(&self, group: PalSoundGroup, index: usize, param: usize) -> i32 {
        let Some(slot) = self.groups.get(&group).and_then(|slots| slots.get(index)) else {
            return 0;
        };
        match param {
            0 => i32::from(slot.data.is_some()),
            1 => i32::from(slot.handle.is_some()),
            2 => i32::from(slot.looping),
            3 => slot.start_ms,
            4 => slot.end_ms,
            _ => 0,
        }
    }

    pub fn sound_status(&self, handle: AudioHandle) -> anyhow::Result<PalSoundStatus> {
        let slot = self.slot(handle)?;
        if slot.data.is_none() {
            return Ok(PalSoundStatus::Free);
        }
        if let Some(sound) = slot.handle.as_ref() {
            return Ok(match sound.state() {
                PlaybackState::Playing => PalSoundStatus::Playing,
                PlaybackState::Pausing
                | PlaybackState::Paused
                | PlaybackState::WaitingToResume
                | PlaybackState::Resuming => PalSoundStatus::Paused,
                PlaybackState::Stopping | PlaybackState::Stopped => PalSoundStatus::Stopped,
            });
        }
        Ok(PalSoundStatus::Loaded)
    }

    pub fn loaded_channel_count_for_handle(&self, handle: AudioHandle) -> usize {
        self.loaded_channel_count(handle.group)
    }

    pub fn group_channel_count(&self, group: PalSoundGroup) -> usize {
        group.slot_count()
    }

    pub fn now_channel_count(&self, group: PalSoundGroup) -> usize {
        self.groups
            .get(&group)
            .map(|slots| {
                slots
                    .iter()
                    .filter(|slot| {
                        slot.handle
                            .as_ref()
                            .is_some_and(|sound| matches!(sound.state(), PlaybackState::Playing))
                    })
                    .count()
            })
            .unwrap_or(0)
    }

    pub fn is_playing(&self, handle: AudioHandle) -> anyhow::Result<bool> {
        let slot = self.slot(handle)?;
        Ok(match slot.handle.as_ref() {
            Some(sound) => matches!(sound.state(), PlaybackState::Playing),
            None => false,
        })
    }

    pub fn loaded_channel_count(&self, group: PalSoundGroup) -> usize {
        self.groups
            .get(&group)
            .map(|slots| slots.iter().filter(|slot| slot.data.is_some()).count())
            .unwrap_or(0)
    }

    pub fn free_channel_count(&self, group: PalSoundGroup) -> usize {
        self.groups
            .get(&group)
            .map(|slots| slots.iter().filter(|slot| slot.data.is_none()).count())
            .unwrap_or(0)
    }

    pub fn update(&mut self) {
        for slots in self.groups.values_mut() {
            for slot in slots {
                if slot
                    .handle
                    .as_ref()
                    .is_some_and(|sound| matches!(sound.state(), PlaybackState::Stopped))
                {
                    slot.handle = None;
                }
            }
        }
    }

    fn find_free_slot(&self, group: PalSoundGroup) -> anyhow::Result<usize> {
        self.groups
            .get(&group)
            .and_then(|slots| slots.iter().position(|slot| slot.data.is_none()))
            .ok_or_else(|| anyhow::anyhow!("no free audio slot in group {:?}", group))
    }

    fn apply_all_volumes(&mut self) -> anyhow::Result<()> {
        for group in PalSoundGroup::ALL {
            let len = self.groups.get(&group).map_or(0, Vec::len);
            for index in 0..len {
                self.apply_slot_volume(AudioHandle::new(group, index))?;
            }
        }
        Ok(())
    }

    fn apply_slot_volume(&mut self, handle: AudioHandle) -> anyhow::Result<()> {
        let effective = self.effective_volume(handle)?;
        let decibels = effective.to_decibels() as f32;
        let slot = self.slot_mut(handle)?;
        let has_loaded_sound = slot.data.is_some() || slot.handle.is_some();
        if has_loaded_sound {
            log::debug!(
                "[trace-audio] apply_slot_volume handle={handle:?} effective_raw={} db={decibels:.2}",
                effective.raw()
            );
        }
        if let Some(sound) = slot.handle.as_mut() {
            sound.set_volume(decibels, Tween::default());
        }
        Ok(())
    }

    pub fn effective_volume(&self, handle: AudioHandle) -> anyhow::Result<PalVolume> {
        let slot = self.slot(handle)?;
        let group = self.group_volume(handle.group);
        Ok(PalVolume::from_raw(
            (self.primary_volume.raw() as i64 * group.raw() as i64 * slot.volume.raw() as i64
                / 10000
                / 10000) as i32,
        ))
    }

    fn slot(&self, handle: AudioHandle) -> anyhow::Result<&AudioSlot> {
        let slots = self
            .groups
            .get(&handle.group)
            .ok_or_else(|| anyhow::anyhow!("invalid audio group {:?}", handle.group))?;
        slots
            .get(handle.index)
            .ok_or_else(|| anyhow::anyhow!("audio handle {:?} points outside its group", handle))
    }

    fn slot_mut(&mut self, handle: AudioHandle) -> anyhow::Result<&mut AudioSlot> {
        let slots = self
            .groups
            .get_mut(&handle.group)
            .ok_or_else(|| anyhow::anyhow!("invalid audio group {:?}", handle.group))?;
        slots
            .get_mut(handle.index)
            .ok_or_else(|| anyhow::anyhow!("audio handle {:?} points outside its group", handle))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PalSoundStatus {
    Free,
    Loaded,
    Playing,
    Paused,
    Stopped,
}

impl PalSoundStatus {
    pub const fn raw(self) -> i32 {
        match self {
            Self::Free => 0,
            Self::Loaded => 1,
            Self::Playing => 2,
            Self::Paused => 3,
            Self::Stopped => 4,
        }
    }
}

#[derive(Default)]
struct AudioSlot {
    name: Option<String>,
    data: Option<StaticSoundData>,
    handle: Option<StaticSoundHandle>,
    looping: bool,
    volume: PalVolume,
    start_ms: i32,
    end_ms: i32,
    pan: i32,
    frequency: i32,
}

fn ensure_loaded(slot: &AudioSlot, handle: AudioHandle) -> anyhow::Result<()> {
    if slot.data.is_some() {
        Ok(())
    } else {
        Err(anyhow::anyhow!(
            "audio handle {:?} has no loaded sound data",
            handle
        ))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct AudioHandle {
    pub group: PalSoundGroup,
    pub index: usize,
}

impl AudioHandle {
    pub fn new(group: PalSoundGroup, index: usize) -> Self {
        Self { group, index }
    }

    pub fn raw(self) -> u32 {
        self.group.raw_prefix() | self.index as u32
    }

    pub fn from_raw(raw: u32) -> Option<Self> {
        let prefix = raw & 0xF000_0000;
        let index = (raw & 0x0FFF_FFFF) as usize;
        PalSoundGroup::from_raw_prefix(prefix).map(|group| Self { group, index })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct PalSoundGroup(pub u8);

impl PalSoundGroup {
    pub const GROUP0: Self = Self(0);
    pub const GROUP1: Self = Self(1);
    pub const GROUP2: Self = Self(2);
    pub const GROUP3: Self = Self(3);
    pub const GROUP4: Self = Self(4);
    pub const GROUP5: Self = Self(5);
    pub const GROUP6: Self = Self(6);
    pub const ALL: [Self; 7] = [
        Self::GROUP0,
        Self::GROUP1,
        Self::GROUP2,
        Self::GROUP3,
        Self::GROUP4,
        Self::GROUP5,
        Self::GROUP6,
    ];

    pub fn from_original_kind(kind: i32) -> Self {
        match kind {
            0 => Self::GROUP0,
            2 => Self::GROUP2,
            3 => Self::GROUP3,
            4 => Self::GROUP4,
            5 => Self::GROUP5,
            6 => Self::GROUP6,
            _ => Self::GROUP1,
        }
    }

    pub fn slot_count(self) -> usize {
        match self.0 {
            0 => 2,
            1 => 16,
            2 => 8,
            3 => 2,
            4 => 16,
            5 => 64,
            6 => 16,
            _ => 0,
        }
    }

    pub fn raw_prefix(self) -> u32 {
        match self.0 {
            0 => 0x1000_0000,
            1 => 0x3000_0000,
            2 => 0x7000_0000,
            3 => 0x2000_0000,
            4 => 0x4000_0000,
            5 => 0x5000_0000,
            6 => 0x6000_0000,
            _ => 0,
        }
    }

    pub fn from_raw_prefix(prefix: u32) -> Option<Self> {
        match prefix {
            0x1000_0000 => Some(Self::GROUP0),
            0x3000_0000 => Some(Self::GROUP1),
            0x7000_0000 => Some(Self::GROUP2),
            0x2000_0000 => Some(Self::GROUP3),
            0x4000_0000 => Some(Self::GROUP4),
            0x5000_0000 => Some(Self::GROUP5),
            0x6000_0000 => Some(Self::GROUP6),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PalVolume(i32);

impl PalVolume {
    pub const MIN: Self = Self(0);
    pub const MAX: Self = Self(10000);

    pub fn from_raw(value: i32) -> Self {
        Self(value).clamped()
    }

    pub fn raw(self) -> i32 {
        self.0
    }

    pub fn clamped(self) -> Self {
        Self(self.0.clamp(Self::MIN.0, Self::MAX.0))
    }

    pub fn to_decibels(self) -> f64 {
        let raw = self.clamped().0;
        if raw == 0 {
            -80.0
        } else {
            20.0 * (raw as f64 / 10000.0).log10()
        }
    }
}

impl Default for PalVolume {
    fn default() -> Self {
        Self::MAX
    }
}
