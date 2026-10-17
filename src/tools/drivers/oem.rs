//! The PC's maker and model, its official driver page and its official update tool.

use crate::tools::backup::sys;

#[derive(Clone, Debug, Default)]
pub struct Pc {
    pub manufacturer: String,
    pub model: String,
    pub serial: String,
    pub board_maker: String,
    pub board: String,
    pub bios: String,
    pub bios_date: String,
}

pub fn pc() -> Pc {
    let script = "$c=Get-CimInstance Win32_ComputerSystem; $b=Get-CimInstance Win32_BIOS; $m=Get-CimInstance Win32_BaseBoard; \
                  @($c.Manufacturer,$c.Model,$b.SerialNumber,$m.Manufacturer,$m.Product,$b.SMBIOSBIOSVersion,$(if($b.ReleaseDate){([datetime]$b.ReleaseDate).ToString('yyyy-MM-dd')})) -join \"`n\"";
    let out = sys::powershell(script).unwrap_or_default();
    let l: Vec<String> = out.lines().map(|s| s.trim().to_string()).collect();
    let g = |i: usize| l.get(i).cloned().unwrap_or_default();
    Pc { manufacturer: g(0), model: g(1), serial: g(2), board_maker: g(3), board: g(4), bios: g(5), bios_date: g(6) }
}

/// An OEM: their driver page and the official tool that keeps their drivers and BIOS current.
#[derive(Clone, Debug, PartialEq)]
pub struct Maker {
    pub name: &'static str,
    pub support_url: String,
    pub tool: Option<(&'static str, &'static str)>,
    pub note: &'static str,
}

/// Filler values firmware uses when the maker didn't set one.
fn placeholder(s: &str) -> bool {
    let l = s.to_lowercase();
    l.is_empty() || l.contains("to be filled") || l.contains("system manufacturer") || l.contains("default string") || l == "o.e.m." || l.contains("not applicable")
}

pub fn maker(pc: &Pc) -> Option<Maker> {
    // Desktops built from parts report the motherboard maker instead.
    let brand = if placeholder(&pc.manufacturer) { &pc.board_maker } else { &pc.manufacturer };
    let b = brand.to_lowercase();
    let model_q = crate::win::shell::url_encode(&format!("{} {}", if placeholder(&pc.model) { &pc.board } else { &pc.model }, ""));
    let serial_ok = !placeholder(&pc.serial) && pc.serial.len() >= 5 && pc.serial.chars().all(|c| c.is_ascii_alphanumeric());
    Some(if b.contains("hewlett") || b == "hp" || b.starts_with("hp ") {
        Maker { name: "HP", support_url: "https://support.hp.com/drivers".into(), tool: Some(("HP Support Assistant", "HP.SupportAssistant")), note: "HP's page detects your PC; or copy the serial number above." }
    } else if b.contains("dell") || b.contains("alienware") {
        Maker {
            name: "Dell",
            support_url: if serial_ok { format!("https://www.dell.com/support/home/product-support/servicetag/{}/drivers", pc.serial) } else { "https://www.dell.com/support/home/drivers".into() },
            tool: Some(("Dell Command | Update", "Dell.CommandUpdate.Universal")),
            note: "Opens the driver list for this exact PC (by its Service Tag).",
        }
    } else if b.contains("lenovo") {
        Maker { name: "Lenovo", support_url: "https://pcsupport.lenovo.com/".into(), tool: Some(("Lenovo System Update", "Lenovo.SystemUpdate")), note: "Lenovo's page detects your PC; or copy the serial number above." }
    } else if b.contains("asus") {
        Maker { name: "ASUS", support_url: format!("https://www.asus.com/support/download-center/?model={model_q}"), tool: Some(("ASUS Armoury Crate", "Asus.ArmouryCrate")), note: "Choose your model in ASUS's Download Center." }
    } else if b.contains("micro-star") || b == "msi" {
        Maker { name: "MSI", support_url: "https://www.msi.com/support/download".into(), tool: Some(("MSI Center", "MSI.MSICenter")), note: "Pick your model on MSI's download page." }
    } else if b.contains("acer") {
        Maker { name: "Acer", support_url: "https://www.acer.com/support/drivers-and-manuals".into(), tool: None, note: "Enter the serial number above on Acer's page." }
    } else if b.contains("gigabyte") {
        Maker { name: "Gigabyte", support_url: "https://www.gigabyte.com/Support".into(), tool: None, note: "Search for your motherboard model on Gigabyte's support page." }
    } else if b.contains("asrock") {
        Maker { name: "ASRock", support_url: "https://www.asrock.com/support/index.asp".into(), tool: None, note: "Search for your motherboard model on ASRock's support page." }
    } else if b.contains("microsoft") {
        Maker { name: "Microsoft Surface", support_url: "https://support.microsoft.com/surface/download-drivers-and-firmware-for-surface".into(), tool: None, note: "Surface drivers and firmware come through Windows Update too." }
    } else if b.contains("samsung") {
        Maker { name: "Samsung", support_url: "https://www.samsung.com/support/".into(), tool: None, note: "Enter your model on Samsung's support page." }
    } else if b.contains("razer") {
        Maker { name: "Razer", support_url: "https://mysupport.razer.com/app/answers/detail/a_id/1828".into(), tool: None, note: "Razer's driver and firmware downloads for Blade laptops." }
    } else if b.contains("framework") {
        Maker { name: "Framework", support_url: "https://knowledgebase.frame.work/".into(), tool: None, note: "Framework's driver bundles for each laptop generation." }
    } else if b.contains("vmware") || b.contains("innotek") || b.contains("qemu") || b.contains("parallels") {
        Maker { name: "Virtual machine", support_url: String::new(), tool: None, note: "This is a virtual machine. Install the hypervisor's guest tools for its drivers." }
    } else {
        return None;
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_makers() {
        let hp = Pc { manufacturer: "HP".into(), model: "OMEN by HP Gaming Laptop 16-n0xxx".into(), ..Default::default() };
        assert_eq!(maker(&hp).unwrap().name, "HP");
        let dell = Pc { manufacturer: "Dell Inc.".into(), model: "XPS 15 9520".into(), serial: "ABC1234".into(), ..Default::default() };
        assert!(maker(&dell).unwrap().support_url.contains("/servicetag/ABC1234/"));
        let diy = Pc { manufacturer: "To Be Filled By O.E.M.".into(), board_maker: "ASRock".into(), board: "B550M Pro4".into(), ..Default::default() };
        assert_eq!(maker(&diy).unwrap().name, "ASRock");
        let msi = Pc { manufacturer: "Micro-Star International Co., Ltd.".into(), ..Default::default() };
        assert_eq!(maker(&msi).unwrap().name, "MSI");
    }

    #[test]
    fn reads_this_pc() {
        let p = pc();
        assert!(!p.manufacturer.is_empty() && !p.bios.is_empty(), "{p:?}");
    }
}
