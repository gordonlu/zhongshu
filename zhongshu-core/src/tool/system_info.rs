use crate::tool::{Tool, ToolOutput};
use async_trait::async_trait;
use serde_json::json;
use sysinfo::{Disks, Networks, System};

pub struct SystemInfoTool;

#[async_trait]
impl Tool for SystemInfoTool {
    fn name(&self) -> &str { "system_info" }

    fn description(&self) -> &str {
        "Collect system information (OS, CPU, memory, disks, network, processes, uptime) using native APIs. No shell required."
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {},
            "required": []
        })
    }

    async fn execute(&self, _arguments: &serde_json::Value) -> ToolOutput {
        let system = System::new_all();

        let hostname = System::host_name().unwrap_or_default();
        let kernel = System::kernel_version().unwrap_or_default();
        let os = System::os_version().unwrap_or_default();
        let uptime = System::uptime();

        let cpus: Vec<serde_json::Value> = system.cpus().iter().map(|cpu| json!({
            "name": cpu.name(),
            "brand": cpu.brand(),
            "usage_pct": cpu.cpu_usage(),
            "frequency_mhz": cpu.frequency(),
        })).collect();
        let cpu_count = system.cpus().len();
        let cpu_usage: f32 = system.global_cpu_usage();

        let total_mem = system.total_memory();
        let used_mem = system.used_memory();
        let free_mem = system.free_memory();
        let total_swap = system.total_swap();
        let used_swap = system.used_swap();

        let disks = Disks::new_with_refreshed_list();
        let disk_list: Vec<serde_json::Value> = disks.iter().map(|d| json!({
            "mount": d.mount_point(),
            "total_bytes": d.total_space(),
            "available_bytes": d.available_space(),
            "file_system": format!("{}", d.file_system().to_string_lossy()),
            "kind": format!("{:?}", d.kind()),
        })).collect();

        let networks = Networks::new_with_refreshed_list();
        let net_list: Vec<serde_json::Value> = networks.iter().map(|(name, data)| json!({
            "interface": name,
            "received_bytes": data.total_received(),
            "transmitted_bytes": data.total_transmitted(),
        })).collect();

        let load = System::load_average();

        let proc_count = system.processes().len();

        ToolOutput::success(json!({
            "hostname": hostname,
            "os": os,
            "kernel": kernel,
            "uptime_secs": uptime,
            "cpu": {
                "count": cpu_count,
                "usage_pct": cpu_usage,
                "cores": cpus,
            },
            "memory": {
                "total_bytes": total_mem,
                "used_bytes": used_mem,
                "free_bytes": free_mem,
                "total_swap_bytes": total_swap,
                "used_swap_bytes": used_swap,
            },
            "disks": disk_list,
            "network": net_list,
            "load_average": {
                "one": load.one,
                "five": load.five,
                "fifteen": load.fifteen,
            },
            "process_count": proc_count,
        }))
    }
}
