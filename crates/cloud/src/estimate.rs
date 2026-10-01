//! What a box costs on Cloudflare Containers (prices as published
//! 2026-10-01: developers.cloudflare.com/containers/pricing).
//!
//! Memory and disk are billed while the container runs; CPU only for active
//! use. The estimate assumes the CPU is busy the whole time it runs, so it's
//! an upper bound. The Workers Paid plan ($5/month) includes an allowance
//! shared by everything in the account; it's subtracted here as if this box
//! were the only user.

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstanceType {
    pub name: &'static str,
    pub vcpu: f64,
    pub memory_gib: f64,
    pub disk_gb: f64,
}

pub const INSTANCE_TYPES: [InstanceType; 6] = [
    InstanceType {
        name: "lite",
        vcpu: 1.0 / 16.0,
        memory_gib: 0.25,
        disk_gb: 2.0,
    },
    InstanceType {
        name: "basic",
        vcpu: 0.25,
        memory_gib: 1.0,
        disk_gb: 4.0,
    },
    InstanceType {
        name: "standard-1",
        vcpu: 0.5,
        memory_gib: 4.0,
        disk_gb: 8.0,
    },
    InstanceType {
        name: "standard-2",
        vcpu: 1.0,
        memory_gib: 6.0,
        disk_gb: 12.0,
    },
    InstanceType {
        name: "standard-3",
        vcpu: 2.0,
        memory_gib: 8.0,
        disk_gb: 16.0,
    },
    InstanceType {
        name: "standard-4",
        vcpu: 4.0,
        memory_gib: 12.0,
        disk_gb: 20.0,
    },
];

pub fn instance_type(name: &str) -> Option<InstanceType> {
    INSTANCE_TYPES.iter().copied().find(|t| t.name == name)
}

const MEMORY_USD_PER_GIB_SECOND: f64 = 0.000_002_5;
const CPU_USD_PER_VCPU_SECOND: f64 = 0.000_020;
const DISK_USD_PER_GB_SECOND: f64 = 0.000_000_07;
const INCLUDED_MEMORY_GIB_HOURS: f64 = 25.0;
const INCLUDED_VCPU_MINUTES: f64 = 375.0;
const INCLUDED_DISK_GB_HOURS: f64 = 200.0;
pub const WORKERS_PAID_USD: f64 = 5.0;

/// A month's cost for one box, in US dollars.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Estimate {
    pub instance_type: InstanceType,
    pub active_hours: f64,
    pub memory_usd: f64,
    pub cpu_usd: f64,
    pub disk_usd: f64,
    /// Usage beyond the plan's allowance (excludes the plan itself).
    pub usage_usd: f64,
    /// The Workers Paid subscription.
    pub plan_usd: f64,
}

pub fn estimate(instance_type: &str, active_hours_per_month: f64) -> Option<Estimate> {
    let t = self::instance_type(instance_type)?;
    let hours = active_hours_per_month.max(0.0);
    let over = |used: f64, included: f64| (used - included).max(0.0);
    let memory_usd =
        over(t.memory_gib * hours, INCLUDED_MEMORY_GIB_HOURS) * 3600.0 * MEMORY_USD_PER_GIB_SECOND;
    let cpu_usd =
        over(t.vcpu * hours * 60.0, INCLUDED_VCPU_MINUTES) * 60.0 * CPU_USD_PER_VCPU_SECOND;
    let disk_usd =
        over(t.disk_gb * hours, INCLUDED_DISK_GB_HOURS) * 3600.0 * DISK_USD_PER_GB_SECOND;
    Some(Estimate {
        instance_type: t,
        active_hours: hours,
        memory_usd,
        cpu_usd,
        disk_usd,
        usage_usd: memory_usd + cpu_usd + disk_usd,
        plan_usd: WORKERS_PAID_USD,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn standard_2_for_100_hours() {
        let e = estimate("standard-2", 100.0).unwrap();
        // memory: (600 - 25) GiB-h * 3600 * 0.0000025 = 5.175
        assert!((e.memory_usd - 5.175).abs() < 1e-9);
        // cpu: (6000 - 375) vCPU-min * 60 * 0.00002 = 6.75
        assert!((e.cpu_usd - 6.75).abs() < 1e-9);
        // disk: (1200 - 200) GB-h * 3600 * 0.00000007 = 0.252
        assert!((e.disk_usd - 0.252).abs() < 1e-9);
        assert_eq!(e.plan_usd, 5.0);
    }

    #[test]
    fn small_use_fits_the_allowance_and_unknown_types_have_none() {
        assert_eq!(estimate("lite", 10.0).unwrap().usage_usd, 0.0);
        assert!(estimate("huge", 1.0).is_none());
    }
}
