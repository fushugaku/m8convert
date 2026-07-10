pub(super) const M8_BOOT_TICK_COUNTER: u32 = 0x2003_a1d0;
pub(super) const SYSTICK_CTRL: u32 = 0xe000_e010;
pub(super) const SYSTICK_LOAD: u32 = 0xe000_e014;
pub(super) const SYSTICK_VALUE: u32 = 0xe000_e018;
pub(super) const SYSTICK_CALIB: u32 = 0xe000_e01c;
pub(super) const DWT_CYCCNT: u32 = 0xe000_1004;
pub(super) const NVIC_ISER_BASE: u32 = 0xe000_e100;
pub(super) const NVIC_ICER_BASE: u32 = 0xe000_e180;
pub(super) const NVIC_ISPR_BASE: u32 = 0xe000_e200;
pub(super) const NVIC_ICPR_BASE: u32 = 0xe000_e280;
pub(super) const NVIC_IABR_BASE: u32 = 0xe000_e300;
pub(super) const NVIC_IPR_BASE: u32 = 0xe000_e400;
pub(super) const NVIC_STIR: u32 = 0xe000_ef00;
pub(super) const SCB_CPUID: u32 = 0xe000_ed00;
pub(super) const SCB_ICSR: u32 = 0xe000_ed04;
pub(super) const SCB_ICSR_VECTACTIVE_MASK: u32 = 0x0000_01ff;
pub(super) const SCB_ICSR_RETTOBASE: u32 = 1 << 11;
pub(super) const SCB_ICSR_VECTPENDING_SHIFT: u32 = 12;
pub(super) const SCB_ICSR_ISRPENDING: u32 = 1 << 22;
pub(super) const SCB_ICSR_PENDSTCLR: u32 = 1 << 25;
pub(super) const SCB_ICSR_PENDSTSET: u32 = 1 << 26;
pub(super) const SCB_ICSR_PENDSVCLR: u32 = 1 << 27;
pub(super) const SCB_ICSR_PENDSVSET: u32 = 1 << 28;
pub(super) const SCB_ICSR_PENDING_MASK: u32 = SCB_ICSR_PENDSTSET | SCB_ICSR_PENDSVSET;
pub(super) const SCB_VTOR: u32 = 0xe000_ed08;
pub(super) const SCB_AIRCR: u32 = 0xe000_ed0c;
pub(super) const SCB_SCR: u32 = 0xe000_ed10;
pub(super) const SCB_CCR: u32 = 0xe000_ed14;
pub(super) const SCB_SHPR1: u32 = 0xe000_ed18;
pub(super) const SCB_SHPR2: u32 = 0xe000_ed1c;
pub(super) const SCB_SHPR3: u32 = 0xe000_ed20;
pub(super) const SCB_SHCSR: u32 = 0xe000_ed24;
pub(super) const SCB_CFSR: u32 = 0xe000_ed28;
pub(super) const SCB_HFSR: u32 = 0xe000_ed2c;
pub(super) const SCB_DFSR: u32 = 0xe000_ed30;
pub(super) const SCB_MMFAR: u32 = 0xe000_ed34;
pub(super) const SCB_BFAR: u32 = 0xe000_ed38;
pub(super) const SCB_AFSR: u32 = 0xe000_ed3c;
pub(super) const SCB_CPACR: u32 = 0xe000_ed88;
pub(super) const MPU_TYPE: u32 = 0xe000_ed90;
pub(super) const MPU_CTRL: u32 = 0xe000_ed94;
pub(super) const MPU_RNR: u32 = 0xe000_ed98;
pub(super) const MPU_RBAR: u32 = 0xe000_ed9c;
pub(super) const MPU_RASR: u32 = 0xe000_eda0;
pub(super) const DCDC_BASE: u32 = 0x4008_0000;
pub(super) const DCDC_END: u32 = 0x4008_4000;
pub(super) const PIT_BASE: u32 = 0x4008_4000;
pub(super) const PIT_CHANNEL_BASE: u32 = 0x4008_4100;
pub(super) const PIT_CHANNEL_STRIDE: u32 = 0x10;
pub(super) const PIT_CHANNEL_COUNT: u32 = 4;
pub(super) const CCM_BASE: u32 = 0x400f_c000;
pub(super) const CCM_END: u32 = 0x4010_0000;
pub(super) const CCM_CS1CDR: u32 = 0x400f_c028;
pub(super) const CCM_CS2CDR: u32 = 0x400f_c02c;
pub(super) const CCM_CCGR5: u32 = 0x400f_c07c;
pub(super) const CCM_CCGR6: u32 = 0x400f_c080;
pub(super) const CCM_CDHIPR: u32 = 0x400f_c048;
pub(super) const CCM_ANALOG_BASE: u32 = 0x400d_8000;
pub(super) const CCM_ANALOG_END: u32 = 0x400d_c000;
pub(super) const CCM_ANALOG_PLL_AUDIO: u32 = 0x400d_8070;
pub(super) const CCM_ANALOG_PLL_AUDIO_SET: u32 = 0x400d_8074;
pub(super) const CCM_ANALOG_PLL_AUDIO_CLR: u32 = 0x400d_8078;
pub(super) const CCM_ANALOG_PLL_AUDIO_TOG: u32 = 0x400d_807c;
pub(super) const CCM_ANALOG_PLL_AUDIO_NUM: u32 = 0x400d_8080;
pub(super) const CCM_ANALOG_PLL_AUDIO_DENOM: u32 = 0x400d_8090;
pub(super) const IOMUXC_GPR1: u32 = 0x400a_c004;
pub(super) const WDOG1_BASE: u32 = 0x400b_8000;
pub(super) const WDOG2_BASE: u32 = 0x400d_0000;
pub(super) const WDOG_SIZE: u32 = 0x4000;
pub(super) const DMA0_BASE: u32 = 0x400e_8000;
pub(super) const DMA0_END: u32 = 0x400e_c000;
pub(super) const DMA_CR: u32 = 0x000;
pub(super) const DMA_ERQ: u32 = 0x00c;
pub(super) const DMA_EEI: u32 = 0x014;
pub(super) const DMA_CEEI: u32 = 0x018;
pub(super) const DMA_SEEI: u32 = 0x019;
pub(super) const DMA_CERQ: u32 = 0x01a;
pub(super) const DMA_SERQ: u32 = 0x01b;
pub(super) const DMA_CINT: u32 = 0x01f;
pub(super) const DMA_INT: u32 = 0x024;
pub(super) const DMA_ERR: u32 = 0x02c;
pub(super) const DMA0_TCD_BASE: u32 = 0x400e_9000;
pub(super) const DMA0_TCD_STRIDE: u32 = 0x20;
pub(super) const DMA0_TCD_COUNT: u32 = 32;
pub(super) const DMAMUX_BASE: u32 = 0x400e_c000;
pub(super) const DMAMUX_END: u32 = 0x400f_0000;
pub(super) const FLEXSPI2_BASE: u32 = 0x402a_4000;
pub(super) const FLEXSPI1_BASE: u32 = 0x402a_8000;
pub(super) const FLEXSPI_SIZE: u32 = 0x4000;
pub(super) const USB1_BASE: u32 = 0x402e_0000;
pub(super) const USB2_BASE: u32 = 0x402e_0200;
pub(super) const USB_SIZE: u32 = 0x200;
pub(super) const SPDIF_BASE: u32 = 0x4038_0000;
pub(super) const SPDIF_END: u32 = 0x4038_4000;
pub(super) const SAI1_BASE: u32 = 0x4038_4000;
pub(super) const SAI2_BASE: u32 = 0x4038_8000;
pub(super) const SAI3_BASE: u32 = 0x4038_c000;
pub(super) const SAI_SIZE: u32 = 0x4000;
pub(super) const SAI_TDR0: u32 = 0x20;
pub(super) const FLEXSPI_MCR0: u32 = 0x000;
pub(super) const FLEXSPI_INTR: u32 = 0x014;
pub(super) const FLEXSPI_LUTKEY: u32 = 0x018;
pub(super) const FLEXSPI_LUTCR: u32 = 0x01c;
pub(super) const FLEXSPI_IPCR0: u32 = 0x0a0;
pub(super) const FLEXSPI_IPCR1: u32 = 0x0a4;
pub(super) const FLEXSPI_IPCMD: u32 = 0x0b0;
pub(super) const FLEXSPI_IPRXFSTS: u32 = 0x0bc;
pub(super) const FLEXSPI_LUT_BASE: u32 = 0x200;
pub(super) const FLEXSPI_LUT_END: u32 = 0x400;
pub(super) const USDHC1_BASE: u32 = 0x402c_0000;
pub(super) const USDHC2_BASE: u32 = 0x402c_4000;
pub(super) const USDHC_SIZE: u32 = 0x4000;
pub(super) const USDHC_DS_ADDR: u32 = 0x00;
pub(super) const USDHC_BLK_ATT: u32 = 0x04;
pub(super) const USDHC_CMD_ARG: u32 = 0x08;
pub(super) const USDHC_CMD_XFR_TYP: u32 = 0x0c;
pub(super) const USDHC_CMD_RSP0: u32 = 0x10;
pub(super) const USDHC_CMD_RSP1: u32 = 0x14;
pub(super) const USDHC_CMD_RSP2: u32 = 0x18;
pub(super) const USDHC_CMD_RSP3: u32 = 0x1c;
pub(super) const USDHC_DATA_BUFF_ACC_PORT: u32 = 0x20;
pub(super) const USDHC_PRES_STATE: u32 = 0x24;
pub(super) const USDHC_PROT_CTRL: u32 = 0x28;
pub(super) const USDHC_SYS_CTRL: u32 = 0x2c;
pub(super) const USDHC_INT_STATUS: u32 = 0x30;
pub(super) const USDHC_INT_STATUS_EN: u32 = 0x34;
pub(super) const USDHC_INT_SIGNAL_EN: u32 = 0x38;
pub(super) const USDHC_AUTOCMD12_ERR_STATUS: u32 = 0x3c;
pub(super) const USDHC_HOST_CTRL_CAP: u32 = 0x40;
pub(super) const USDHC_WTMK_LVL: u32 = 0x44;
pub(super) const USDHC_MIX_CTRL: u32 = 0x48;
pub(super) const USDHC_IRQ1: u16 = 110;
pub(super) const USDHC_IRQ2: u16 = 111;
pub(super) const USDHC_SYS_CTRL_RESET_MASK: u32 = 0x0f00_0000;
pub(super) const USDHC_INT_CC: u32 = 1 << 0;
pub(super) const USDHC_INT_TC: u32 = 1 << 1;
pub(super) const USDHC_INT_DINT: u32 = 1 << 3;
pub(super) const USDHC_INT_BWR: u32 = 1 << 4;
pub(super) const USDHC_INT_BRR: u32 = 1 << 5;
pub(super) const USDHC_PRES_SDSTB: u32 = 1 << 3;
pub(super) const USDHC_PRES_BWEN: u32 = 1 << 10;
pub(super) const USDHC_PRES_BREN: u32 = 1 << 11;
pub(super) const USDHC_PRES_CINS: u32 = 1 << 16;
pub(super) const USDHC_PRES_CDPL: u32 = 1 << 18;
pub(super) const USDHC_PRES_WPSPL: u32 = 1 << 19;
pub(super) const USDHC_PRES_CLSL: u32 = 1 << 23;
pub(super) const USDHC_PRES_DLSL: u32 = 0xff << 24;
pub(super) const USDHC_PRES_READY: u32 = USDHC_PRES_SDSTB
    | USDHC_PRES_BWEN
    | USDHC_PRES_BREN
    | USDHC_PRES_CINS
    | USDHC_PRES_CDPL
    | USDHC_PRES_WPSPL
    | USDHC_PRES_DLSL
    | USDHC_PRES_CLSL;
pub(super) const USDHC_HOST_CAP_READY: u32 = 0x07eb_0000;

fn nvic_word_offset(address: u32, base: u32, words: u32) -> Option<u32> {
    if address & 0x03 != 0 {
        return None;
    }
    let end = base + words * 4;
    if (base..end).contains(&address) {
        Some(address - base)
    } else {
        None
    }
}

pub(super) fn nvic_set_enable_target(address: u32) -> Option<u32> {
    nvic_word_offset(address, NVIC_ISER_BASE, 8).map(|offset| NVIC_ISER_BASE + offset)
}

pub(super) fn nvic_clear_enable_target(address: u32) -> Option<u32> {
    nvic_word_offset(address, NVIC_ICER_BASE, 8).map(|offset| NVIC_ISER_BASE + offset)
}

pub(super) fn nvic_enable_mirror_source(address: u32) -> Option<u32> {
    nvic_word_offset(address, NVIC_ICER_BASE, 8).map(|offset| NVIC_ISER_BASE + offset)
}

pub(super) fn nvic_set_pending_target(address: u32) -> Option<u32> {
    nvic_word_offset(address, NVIC_ISPR_BASE, 8).map(|offset| NVIC_ISPR_BASE + offset)
}

pub(super) fn nvic_clear_pending_target(address: u32) -> Option<u32> {
    nvic_word_offset(address, NVIC_ICPR_BASE, 8).map(|offset| NVIC_ISPR_BASE + offset)
}

pub(super) fn nvic_pending_mirror_source(address: u32) -> Option<u32> {
    nvic_word_offset(address, NVIC_ICPR_BASE, 8).map(|offset| NVIC_ISPR_BASE + offset)
}

pub(super) fn is_nvic_active_register(address: u32) -> bool {
    nvic_word_offset(address, NVIC_IABR_BASE, 8).is_some()
}

pub(super) fn pit_channel_address(channel: u32, register_offset: u32) -> u32 {
    PIT_CHANNEL_BASE + channel * PIT_CHANNEL_STRIDE + register_offset
}

pub(super) fn pit_channel_register(address: u32, register_offset: u32) -> Option<u32> {
    if address < PIT_CHANNEL_BASE {
        return None;
    }
    let relative = address - PIT_CHANNEL_BASE;
    let channel = relative / PIT_CHANNEL_STRIDE;
    if channel >= PIT_CHANNEL_COUNT || relative % PIT_CHANNEL_STRIDE != register_offset {
        None
    } else {
        Some(channel)
    }
}

pub(super) fn flexspi_base(address: u32) -> Option<u32> {
    if (FLEXSPI1_BASE..FLEXSPI1_BASE + FLEXSPI_SIZE).contains(&address) {
        Some(FLEXSPI1_BASE)
    } else if (FLEXSPI2_BASE..FLEXSPI2_BASE + FLEXSPI_SIZE).contains(&address) {
        Some(FLEXSPI2_BASE)
    } else {
        None
    }
}

pub(super) fn flexspi_register(address: u32, register_offset: u32) -> Option<u32> {
    let base = flexspi_base(address)?;
    if address == base + register_offset {
        Some(base)
    } else {
        None
    }
}

pub(super) fn flexspi_lut_address(address: u32) -> bool {
    flexspi_base(address)
        .map(|base| {
            let offset = address - base;
            address & 0x03 == 0 && (FLEXSPI_LUT_BASE..FLEXSPI_LUT_END).contains(&offset)
        })
        .unwrap_or(false)
}

pub(super) fn usdhc_base(address: u32) -> Option<u32> {
    if (USDHC1_BASE..USDHC1_BASE + USDHC_SIZE).contains(&address) {
        Some(USDHC1_BASE)
    } else if (USDHC2_BASE..USDHC2_BASE + USDHC_SIZE).contains(&address) {
        Some(USDHC2_BASE)
    } else {
        None
    }
}

pub(super) fn usdhc_offset(address: u32) -> Option<u32> {
    usdhc_base(address).map(|base| address - base)
}

pub(super) fn usdhc_register(address: u32, register_offset: u32) -> Option<u32> {
    usdhc_offset(address).and_then(|offset| (offset == register_offset).then_some(offset))
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(super) enum AudioMmioKind {
    Sai,
    Dma,
    Dmamux,
    Usb,
    Clock,
}

pub(super) fn audio_mmio_kind(address: u32) -> Option<AudioMmioKind> {
    if sai_base(address).is_some() || (SPDIF_BASE..SPDIF_END).contains(&address) {
        return Some(AudioMmioKind::Sai);
    }
    if (DMA0_BASE..DMA0_END).contains(&address) {
        return Some(AudioMmioKind::Dma);
    }
    if (DMAMUX_BASE..DMAMUX_END).contains(&address) {
        return Some(AudioMmioKind::Dmamux);
    }
    if (USB1_BASE..USB1_BASE + USB_SIZE).contains(&address)
        || (USB2_BASE..USB2_BASE + USB_SIZE).contains(&address)
    {
        return Some(AudioMmioKind::Usb);
    }
    if is_audio_clock_register(address) {
        return Some(AudioMmioKind::Clock);
    }
    None
}

pub(super) fn sai_base(address: u32) -> Option<u32> {
    if (SAI1_BASE..SAI1_BASE + SAI_SIZE).contains(&address) {
        Some(SAI1_BASE)
    } else if (SAI2_BASE..SAI2_BASE + SAI_SIZE).contains(&address) {
        Some(SAI2_BASE)
    } else if (SAI3_BASE..SAI3_BASE + SAI_SIZE).contains(&address) {
        Some(SAI3_BASE)
    } else {
        None
    }
}

fn is_audio_clock_register(address: u32) -> bool {
    matches!(
        address,
        CCM_ANALOG_PLL_AUDIO
            | CCM_ANALOG_PLL_AUDIO_SET
            | CCM_ANALOG_PLL_AUDIO_CLR
            | CCM_ANALOG_PLL_AUDIO_TOG
            | CCM_ANALOG_PLL_AUDIO_NUM
            | CCM_ANALOG_PLL_AUDIO_DENOM
            | CCM_CS1CDR
            | CCM_CS2CDR
            | CCM_CCGR5
            | CCM_CCGR6
            | IOMUXC_GPR1
    )
}

#[derive(Debug, Clone, Copy)]
pub(super) enum PeripheralAlias {
    Set,
    Clear,
    Toggle,
}

pub(super) fn peripheral_alias(address: u32) -> Option<(u32, PeripheralAlias)> {
    if !((CCM_ANALOG_BASE..CCM_ANALOG_END).contains(&address)
        || (DCDC_BASE..DCDC_END).contains(&address))
    {
        return None;
    }

    let actual = address & !0x0f;
    match address & 0x0f {
        0x04 => Some((actual, PeripheralAlias::Set)),
        0x08 => Some((actual, PeripheralAlias::Clear)),
        0x0c => Some((actual, PeripheralAlias::Toggle)),
        _ => None,
    }
}

pub(super) fn peripheral_ready_value(address: u32, value: u32) -> u32 {
    if address == CCM_CDHIPR {
        return 0;
    }
    if address == 0x400d_8010 && value & 0x3000 == 0x3000 {
        return value | 0x8000_0000;
    }
    if address == 0x400d_8000 && value != 0 {
        return value | 0x8000_0000;
    }
    if address == 0x400d_8070 && value != 0 {
        return value | 0x8000_0000;
    }
    if address == 0x4008_0000 && value != 0 {
        return value | 0x8000_0000;
    }
    if matches!(address, 0x402a_4000 | 0x402a_8000) {
        return value & !1;
    }
    if matches!(address, 0x400c_4048 | 0x400c_8048) {
        return value & !0x80;
    }
    if usdhc_register(address, USDHC_SYS_CTRL).is_some() {
        return value & !USDHC_SYS_CTRL_RESET_MASK;
    }
    if usdhc_register(address, USDHC_INT_STATUS).is_some() {
        return value;
    }
    if address == 0x4008_410c {
        return 0;
    }
    if flexspi_register(address, FLEXSPI_MCR0).is_some() {
        return value & !1;
    }
    if (CCM_BASE..CCM_END).contains(&address) {
        return value;
    }
    if (CCM_ANALOG_BASE..CCM_ANALOG_END).contains(&address) && address & 0x0f == 0 && value != 0 {
        return value | 0x8000_0000;
    }
    value
}

pub(super) fn peripheral_default_value(address: u32) -> u32 {
    match address {
        SYSTICK_CALIB => 0,
        SCB_CPUID => 0x411f_c271,
        SCB_ICSR => 0,
        SCB_VTOR => 0,
        SCB_AIRCR => 0xfa05_0000,
        SCB_SCR => 0,
        SCB_CCR => 0x0000_0200,
        SCB_SHPR1 | SCB_SHPR2 | SCB_SHPR3 => 0,
        SCB_SHCSR | SCB_CFSR | SCB_HFSR | SCB_DFSR => 0,
        SCB_MMFAR | SCB_BFAR | SCB_AFSR => 0,
        SCB_CPACR => 0,
        MPU_TYPE => 0x0000_0800,
        MPU_CTRL | MPU_RNR | MPU_RBAR | MPU_RASR => 0,
        CCM_CDHIPR => 0,
        address if (PIT_BASE..PIT_BASE + 0x1000).contains(&address) => 0,
        address if flexspi_register(address, FLEXSPI_INTR).is_some() => 1,
        address if flexspi_register(address, FLEXSPI_IPRXFSTS).is_some() => 1,
        address if (NVIC_IPR_BASE..NVIC_IPR_BASE + 240).contains(&address) => 0,
        0x402a_4014 | 0x402a_8014 => 1,
        address if usdhc_register(address, USDHC_PRES_STATE).is_some() => USDHC_PRES_READY,
        address if usdhc_register(address, USDHC_INT_STATUS).is_some() => 0,
        address if usdhc_register(address, USDHC_HOST_CTRL_CAP).is_some() => USDHC_HOST_CAP_READY,
        0x400c_8020 => 1,
        0x4008_410c => 1,
        _ => 0,
    }
}

pub(super) fn mmio_label(address: u32) -> Option<String> {
    let direct = match address {
        SYSTICK_CTRL => "SysTick CTRL",
        SYSTICK_LOAD => "SysTick LOAD",
        SYSTICK_VALUE => "SysTick VAL",
        SYSTICK_CALIB => "SysTick CALIB",
        DWT_CYCCNT => "DWT CYCCNT",
        SCB_CPUID => "SCB CPUID",
        SCB_ICSR => "SCB ICSR",
        SCB_VTOR => "SCB VTOR",
        SCB_AIRCR => "SCB AIRCR",
        SCB_SCR => "SCB SCR",
        SCB_CCR => "SCB CCR",
        SCB_CFSR => "SCB CFSR",
        SCB_HFSR => "SCB HFSR",
        SCB_DFSR => "SCB DFSR",
        SCB_CPACR => "SCB CPACR",
        MPU_TYPE => "MPU TYPE",
        MPU_CTRL => "MPU CTRL",
        MPU_RNR => "MPU RNR",
        MPU_RBAR => "MPU RBAR",
        MPU_RASR => "MPU RASR",
        NVIC_STIR => "NVIC STIR",
        CCM_CDHIPR => "CCM CDHIPR",
        CCM_CS1CDR => "CCM CS1CDR (SAI1/SAI3 clock)",
        CCM_CS2CDR => "CCM CS2CDR (SAI2 clock)",
        CCM_CCGR5 => "CCM CCGR5 (SAI/SPDIF/DMA gates)",
        CCM_CCGR6 => "CCM CCGR6 (USB gates)",
        CCM_ANALOG_PLL_AUDIO => "CCM_ANALOG PLL_AUDIO",
        CCM_ANALOG_PLL_AUDIO_SET => "CCM_ANALOG PLL_AUDIO_SET",
        CCM_ANALOG_PLL_AUDIO_CLR => "CCM_ANALOG PLL_AUDIO_CLR",
        CCM_ANALOG_PLL_AUDIO_TOG => "CCM_ANALOG PLL_AUDIO_TOG",
        CCM_ANALOG_PLL_AUDIO_NUM => "CCM_ANALOG PLL_AUDIO_NUM",
        CCM_ANALOG_PLL_AUDIO_DENOM => "CCM_ANALOG PLL_AUDIO_DENOM",
        IOMUXC_GPR1 => "IOMUXC_GPR GPR1 (SAI routing)",
        _ => "",
    };
    if !direct.is_empty() {
        return Some(direct.to_string());
    }

    if let Some(offset) = nvic_word_offset(address, NVIC_ISER_BASE, 8) {
        return Some(format!("NVIC ISER[{}]", offset / 4));
    }
    if let Some(offset) = nvic_word_offset(address, NVIC_ICER_BASE, 8) {
        return Some(format!("NVIC ICER[{}]", offset / 4));
    }
    if let Some(offset) = nvic_word_offset(address, NVIC_ISPR_BASE, 8) {
        return Some(format!("NVIC ISPR[{}]", offset / 4));
    }
    if let Some(offset) = nvic_word_offset(address, NVIC_ICPR_BASE, 8) {
        return Some(format!("NVIC ICPR[{}]", offset / 4));
    }
    if let Some(offset) = nvic_word_offset(address, NVIC_IABR_BASE, 8) {
        return Some(format!("NVIC IABR[{}]", offset / 4));
    }
    if (NVIC_IPR_BASE..NVIC_IPR_BASE + 240).contains(&address) {
        return Some(format!("NVIC IPR+0x{:02x}", address - NVIC_IPR_BASE));
    }
    if let Some(base) = flexspi_base(address) {
        let instance = if base == FLEXSPI1_BASE {
            "FlexSPI1"
        } else {
            "FlexSPI2"
        };
        return Some(format!(
            "{instance} {}",
            flexspi_register_name(address - base)
        ));
    }
    if let Some(base) = usdhc_base(address) {
        let instance = if base == USDHC1_BASE {
            "USDHC1"
        } else {
            "USDHC2"
        };
        return Some(format!(
            "{instance} {}",
            usdhc_register_name(address - base)
        ));
    }
    if let Some(base) = sai_base(address) {
        let instance = match base {
            SAI1_BASE => "SAI1",
            SAI2_BASE => "SAI2",
            SAI3_BASE => "SAI3",
            _ => "SAI",
        };
        return Some(format!("{instance} {}", sai_register_name(address - base)));
    }
    if (SPDIF_BASE..SPDIF_END).contains(&address) {
        return Some(format!("SPDIF+0x{:03x}", address - SPDIF_BASE));
    }
    if (DMA0_BASE..DMA0_END).contains(&address) {
        return Some(format!("DMA0 {}", dma_register_name(address - DMA0_BASE)));
    }
    if (DMAMUX_BASE..DMAMUX_END).contains(&address) {
        return Some(dmamux_register_name(address - DMAMUX_BASE));
    }
    if (USB1_BASE..USB1_BASE + USB_SIZE).contains(&address) {
        return Some(format!("USB1 {}", usb_register_name(address - USB1_BASE)));
    }
    if (USB2_BASE..USB2_BASE + USB_SIZE).contains(&address) {
        return Some(format!("USB2 {}", usb_register_name(address - USB2_BASE)));
    }
    if (PIT_BASE..PIT_BASE + 0x1000).contains(&address) {
        return Some(format!("PIT+0x{:03x}", address - PIT_BASE));
    }
    if (CCM_BASE..CCM_END).contains(&address) {
        return Some(format!("CCM+0x{:03x}", address - CCM_BASE));
    }
    if (CCM_ANALOG_BASE..CCM_ANALOG_END).contains(&address) {
        return Some(format!("CCM_ANALOG+0x{:03x}", address - CCM_ANALOG_BASE));
    }
    if (DCDC_BASE..DCDC_END).contains(&address) {
        return Some(format!("DCDC+0x{:03x}", address - DCDC_BASE));
    }
    if (WDOG1_BASE..WDOG1_BASE + WDOG_SIZE).contains(&address) {
        return Some(format!(
            "WDOG1 {}",
            wdog_register_name(address - WDOG1_BASE)
        ));
    }
    if (WDOG2_BASE..WDOG2_BASE + WDOG_SIZE).contains(&address) {
        return Some(format!(
            "WDOG2 {}",
            wdog_register_name(address - WDOG2_BASE)
        ));
    }
    if (0xe000_ef00..0xe000_f000).contains(&address) {
        return Some(format!("FPU/SCS+0x{:03x}", address - 0xe000_ef00));
    }

    None
}

fn flexspi_register_name(offset: u32) -> String {
    match offset {
        FLEXSPI_MCR0 => "MCR0".to_string(),
        FLEXSPI_INTR => "INTR".to_string(),
        FLEXSPI_LUTKEY => "LUTKEY".to_string(),
        FLEXSPI_LUTCR => "LUTCR".to_string(),
        FLEXSPI_IPCR0 => "IPCR0".to_string(),
        FLEXSPI_IPCR1 => "IPCR1".to_string(),
        FLEXSPI_IPCMD => "IPCMD".to_string(),
        FLEXSPI_IPRXFSTS => "IPRXFSTS".to_string(),
        offset if (FLEXSPI_LUT_BASE..FLEXSPI_LUT_END).contains(&offset) => {
            format!("LUT+0x{:03x}", offset - FLEXSPI_LUT_BASE)
        }
        _ => format!("+0x{offset:03x}"),
    }
}

fn usdhc_register_name(offset: u32) -> String {
    match offset {
        USDHC_DS_ADDR => "DS_ADDR".to_string(),
        USDHC_BLK_ATT => "BLK_ATT".to_string(),
        USDHC_CMD_ARG => "CMD_ARG".to_string(),
        USDHC_CMD_XFR_TYP => "CMD_XFR_TYP".to_string(),
        USDHC_CMD_RSP0 => "CMD_RSP0".to_string(),
        USDHC_CMD_RSP1 => "CMD_RSP1".to_string(),
        USDHC_CMD_RSP2 => "CMD_RSP2".to_string(),
        USDHC_CMD_RSP3 => "CMD_RSP3".to_string(),
        USDHC_DATA_BUFF_ACC_PORT => "DATA_BUFF_ACC_PORT".to_string(),
        USDHC_PRES_STATE => "PRES_STATE".to_string(),
        USDHC_PROT_CTRL => "PROT_CTRL".to_string(),
        USDHC_SYS_CTRL => "SYS_CTRL".to_string(),
        USDHC_INT_STATUS => "INT_STATUS".to_string(),
        USDHC_INT_STATUS_EN => "INT_STATUS_EN".to_string(),
        USDHC_INT_SIGNAL_EN => "INT_SIGNAL_EN".to_string(),
        USDHC_AUTOCMD12_ERR_STATUS => "AUTOCMD12_ERR_STATUS".to_string(),
        USDHC_HOST_CTRL_CAP => "HOST_CTRL_CAP".to_string(),
        USDHC_WTMK_LVL => "WTMK_LVL".to_string(),
        USDHC_MIX_CTRL => "MIX_CTRL".to_string(),
        _ => format!("+0x{offset:03x}"),
    }
}

fn sai_register_name(offset: u32) -> String {
    match offset {
        0x00 => "VERID".to_string(),
        0x04 => "PARAM".to_string(),
        0x08 => "TCSR".to_string(),
        0x0c => "TCR1".to_string(),
        0x10 => "TCR2".to_string(),
        0x14 => "TCR3".to_string(),
        0x18 => "TCR4".to_string(),
        0x1c => "TCR5".to_string(),
        0x20..=0x2f => format!("TDR{}+0x{:x}", (offset - 0x20) / 4, offset & 0x03),
        0x40..=0x4f => format!("TFR{}+0x{:x}", (offset - 0x40) / 4, offset & 0x03),
        0x60 => "TMR".to_string(),
        0x88 => "RCSR".to_string(),
        0x8c => "RCR1".to_string(),
        0x90 => "RCR2".to_string(),
        0x94 => "RCR3".to_string(),
        0x98 => "RCR4".to_string(),
        0x9c => "RCR5".to_string(),
        0xa0..=0xaf => format!("RDR{}+0x{:x}", (offset - 0xa0) / 4, offset & 0x03),
        0xc0..=0xcf => format!("RFR{}+0x{:x}", (offset - 0xc0) / 4, offset & 0x03),
        0xe0 => "RMR".to_string(),
        _ => format!("+0x{offset:03x}"),
    }
}

fn dma_register_name(offset: u32) -> String {
    let tcd_base_offset = DMA0_TCD_BASE - DMA0_BASE;
    if (tcd_base_offset..tcd_base_offset + DMA0_TCD_COUNT * DMA0_TCD_STRIDE).contains(&offset) {
        let relative = offset - tcd_base_offset;
        let channel = relative / DMA0_TCD_STRIDE;
        let field_offset = relative % DMA0_TCD_STRIDE;
        return format!("TCD{channel} {}", dma_tcd_field_name(field_offset));
    }

    match offset {
        DMA_CR => "CR".to_string(),
        0x004 => "ES".to_string(),
        DMA_ERQ => "ERQ".to_string(),
        DMA_EEI => "EEI".to_string(),
        DMA_CEEI => "CEEI".to_string(),
        DMA_SEEI => "SEEI".to_string(),
        DMA_CERQ => "CERQ".to_string(),
        DMA_SERQ => "SERQ".to_string(),
        0x01c => "CDNE".to_string(),
        0x01d => "SSRT".to_string(),
        0x01e => "CERR".to_string(),
        DMA_CINT => "CINT".to_string(),
        DMA_INT => "INT".to_string(),
        DMA_ERR => "ERR".to_string(),
        0x034 => "HRS".to_string(),
        0x100..=0x17f => format!("DCHPRI{}", (offset - 0x100) / 4),
        _ => format!("+0x{offset:03x}"),
    }
}

fn dma_tcd_field_name(offset: u32) -> String {
    match offset {
        0x00..=0x03 => format!("SADDR+0x{:x}", offset & 0x03),
        0x04..=0x05 => format!("SOFF+0x{:x}", offset - 0x04),
        0x06..=0x07 => format!("ATTR+0x{:x}", offset - 0x06),
        0x08..=0x0b => format!("NBYTES+0x{:x}", offset - 0x08),
        0x0c..=0x0f => format!("SLAST+0x{:x}", offset - 0x0c),
        0x10..=0x13 => format!("DADDR+0x{:x}", offset - 0x10),
        0x14..=0x15 => format!("DOFF+0x{:x}", offset - 0x14),
        0x16..=0x17 => format!("CITER+0x{:x}", offset - 0x16),
        0x18..=0x1b => format!("DLAST_SGA+0x{:x}", offset - 0x18),
        0x1c..=0x1d => format!("CSR+0x{:x}", offset - 0x1c),
        0x1e..=0x1f => format!("BITER+0x{:x}", offset - 0x1e),
        _ => format!("+0x{offset:02x}"),
    }
}

fn dmamux_register_name(offset: u32) -> String {
    if offset < 0x80 {
        return format!("DMAMUX CHCFG{}", offset / 4);
    }
    format!("DMAMUX+0x{offset:03x}")
}

fn usb_register_name(offset: u32) -> String {
    match offset {
        0x000 => "ID".to_string(),
        0x004 => "HWGENERAL".to_string(),
        0x008 => "HWHOST".to_string(),
        0x00c => "HWDEVICE".to_string(),
        0x010 => "HWTXBUF".to_string(),
        0x014 => "HWRXBUF".to_string(),
        0x140 => "USBCMD".to_string(),
        0x144 => "USBSTS".to_string(),
        0x148 => "USBINTR".to_string(),
        0x14c => "FRINDEX".to_string(),
        0x158 => "ENDPOINTLISTADDR/ASYNCLISTADDR".to_string(),
        0x184 => "PORTSC1".to_string(),
        0x1a8 => "USBMODE".to_string(),
        0x1ac => "ENDPTSETUPSTAT".to_string(),
        0x1b0 => "ENDPTPRIME".to_string(),
        0x1b4 => "ENDPTFLUSH".to_string(),
        0x1b8 => "ENDPTSTATUS".to_string(),
        0x1bc => "ENDPTCOMPLETE".to_string(),
        0x1c0..=0x1df => format!("ENDPTCTRL{}", (offset - 0x1c0) / 4),
        _ => format!("+0x{offset:03x}"),
    }
}

fn wdog_register_name(offset: u32) -> String {
    match offset {
        0x00 => "WCR".to_string(),
        0x02 => "WSR".to_string(),
        0x04 => "WRSR".to_string(),
        0x06 => "WICR".to_string(),
        0x08 => "WMCR".to_string(),
        _ => format!("+0x{offset:03x}"),
    }
}
