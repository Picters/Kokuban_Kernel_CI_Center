use anyhow::{Result, anyhow};
use chrono::{FixedOffset, Utc};
use std::collections::{HashMap, HashSet};
use std::env;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use crate::config::{AnyKernelConfig, BbgConfig, OotModule, ProjectConfig, SusfsConfig};
use crate::utils::{
    cache_file_name, command_exists, env_flag, file_sha256, handle_notify, is_resukisu_variant,
    load_anykernel_config, load_project, run_cmd, run_cmd_with_env, set_github_output,
    url_file_name, variant_suffix,
};

const ANYKERNEL_REPO: &str = "https://github.com/YuzakiKokuban/AnyKernel3.git";
const ANYKERNEL_BRANCH: &str = "master";

fn verify_toolchain_checksum(
    url: &str,
    path: &Path,
    checksums: Option<&HashMap<String, String>>,
) -> Result<()> {
    let Some(expected) = checksums.and_then(|items| items.get(url)) else {
        return Ok(());
    };

    let actual = file_sha256(path)?;
    if !actual.eq_ignore_ascii_case(expected) {
        return Err(anyhow!(
            "Toolchain checksum mismatch for {}\nexpected: {}\nactual:   {}",
            url,
            expected,
            actual
        ));
    }

    Ok(())
}

fn toolchain_paths_ready(toolchain_base: &Path, proj: &ProjectConfig) -> bool {
    if let Some(exports) = &proj.toolchain_path_exports {
        return exports
            .iter()
            .all(|export| toolchain_base.join(export).exists());
    }

    !proj
        .toolchain_path_prefix
        .as_deref()
        .unwrap_or("")
        .is_empty()
        && toolchain_base.join("bin").exists()
}

fn download_toolchains(
    urls: &[String],
    tc_download_dir: &Path,
    cache_dir: Option<&Path>,
    offline: bool,
    checksums: Option<&HashMap<String, String>>,
) -> Result<()> {
    if tc_download_dir.exists() {
        fs::remove_dir_all(tc_download_dir)?;
    }
    fs::create_dir_all(tc_download_dir)?;

    for url in urls {
        let file_name = url_file_name(url)?;
        let download_path = tc_download_dir.join(&file_name);

        if let Some(cache_dir) = cache_dir {
            fs::create_dir_all(cache_dir)?;
            let cache_path = cache_dir.join(cache_file_name(url)?);

            if cache_path.exists() {
                verify_toolchain_checksum(url, &cache_path, checksums)?;
                println!("Using cached toolchain package: {}", file_name);
                fs::copy(cache_path, download_path)?;
                continue;
            }

            if offline {
                return Err(anyhow!(
                    "Toolchain package is missing from cache while offline: {}",
                    url
                ));
            }

            println!("Downloading toolchain from {}...", url);
            let cache_file_name = cache_path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("toolchain");
            let tmp_path = cache_path.with_file_name(format!(
                "{}.tmp-{}",
                cache_file_name,
                std::process::id()
            ));
            let _ = fs::remove_file(&tmp_path);
            let tmp_path_str = tmp_path
                .to_str()
                .ok_or_else(|| anyhow!("Invalid toolchain cache path"))?;
            let download_result = (|| {
                run_cmd(&["wget", "-q", "-O", tmp_path_str, url], None, false)?;
                verify_toolchain_checksum(url, &tmp_path, checksums)?;
                fs::rename(&tmp_path, &cache_path)?;
                Ok(())
            })();
            if let Err(err) = download_result {
                let _ = fs::remove_file(&tmp_path);
                return Err(err);
            }
            fs::copy(cache_path, download_path)?;
        } else {
            if offline {
                return Err(anyhow!(
                    "Toolchain cache is disabled and offline mode cannot download: {}",
                    url
                ));
            }

            println!("Downloading toolchain from {}...", url);
            let download_path_str = download_path
                .to_str()
                .ok_or_else(|| anyhow!("Invalid toolchain download path"))?;
            run_cmd(&["wget", "-q", "-O", download_path_str, url], None, false)?;
            verify_toolchain_checksum(url, &download_path, checksums)?;
        }
    }

    Ok(())
}

fn prepare_anykernel_cache(cache_dir: &Path, offline: bool) -> Result<()> {
    if !cache_dir.join(".git").exists() {
        if offline {
            return Err(anyhow!(
                "AnyKernel3 cache is missing while offline: {}",
                cache_dir.display()
            ));
        }
        fs::create_dir_all(
            cache_dir
                .parent()
                .ok_or_else(|| anyhow!("Invalid AnyKernel3 cache path"))?,
        )?;
        run_cmd(
            &[
                "git",
                "clone",
                ANYKERNEL_REPO,
                "-b",
                ANYKERNEL_BRANCH,
                cache_dir
                    .to_str()
                    .ok_or_else(|| anyhow!("Invalid AnyKernel3 cache path"))?,
            ],
            None,
            false,
        )?;
    } else if !offline {
        run_cmd(
            &["git", "fetch", "--tags", "--prune", "origin"],
            Some(cache_dir),
            false,
        )?;
    }

    run_cmd(
        &["git", "checkout", "--force", ANYKERNEL_BRANCH],
        Some(cache_dir),
        false,
    )?;

    if !offline {
        let remote_branch = format!("origin/{ANYKERNEL_BRANCH}");
        run_cmd(
            &["git", "reset", "--hard", &remote_branch],
            Some(cache_dir),
            false,
        )?;
    } else {
        run_cmd(&["git", "reset", "--hard", "HEAD"], Some(cache_dir), false)?;
    }

    run_cmd(&["git", "clean", "-ffdx"], Some(cache_dir), false)?;
    Ok(())
}

fn prepare_anykernel_worktree(target_dir: &Path, offline: bool) -> Result<()> {
    if target_dir.exists() {
        fs::remove_dir_all(target_dir)?;
    }

    if let Ok(cache_dir) = env::var("KOKUBAN_ANYKERNEL_CACHE_DIR") {
        let cache_dir = PathBuf::from(cache_dir);
        prepare_anykernel_cache(&cache_dir, offline)?;
        run_cmd(
            &[
                "git",
                "clone",
                "--shared",
                cache_dir
                    .to_str()
                    .ok_or_else(|| anyhow!("Invalid AnyKernel3 cache path"))?,
                target_dir
                    .to_str()
                    .ok_or_else(|| anyhow!("Invalid AnyKernel3 target path"))?,
            ],
            None,
            false,
        )?;
        run_cmd(
            &["git", "checkout", "--force", ANYKERNEL_BRANCH],
            Some(target_dir),
            false,
        )?;
    } else {
        if offline {
            return Err(anyhow!(
                "KOKUBAN_ANYKERNEL_CACHE_DIR is required for offline AnyKernel3 setup"
            ));
        }

        run_cmd(
            &[
                "git",
                "clone",
                ANYKERNEL_REPO,
                "-b",
                ANYKERNEL_BRANCH,
                target_dir
                    .to_str()
                    .ok_or_else(|| anyhow!("Invalid AnyKernel3 target path"))?,
            ],
            None,
            false,
        )?;
    }

    Ok(())
}

fn create_compiler_wrapper(
    wrapper_dir: &Path,
    wrapper_name: &str,
    command_prefix: &str,
    tool: &str,
) -> Result<String> {
    let wrapper_path = wrapper_dir.join(wrapper_name);
    fs::write(
        &wrapper_path,
        format!("#!/bin/sh\nexec {} {} \"$@\"\n", command_prefix, tool),
    )?;
    fs::set_permissions(&wrapper_path, PermissionsExt::from_mode(0o755))?;
    Ok(wrapper_path.to_string_lossy().to_string())
}

fn copy_artifact_if_exists(source: &Path, artifact_dir: &Path) -> Result<bool> {
    if !source.is_file() {
        return Ok(false);
    }

    let file_name = source
        .file_name()
        .ok_or_else(|| anyhow!("Artifact path {:?} has no filename", source))?;
    fs::copy(source, artifact_dir.join(file_name))?;
    Ok(true)
}

fn upsert_kconfig_entry(content: &str, key: &str, value: &str) -> String {
    let key_prefix = format!("{key}=");
    let not_set_line = format!("# {key} is not set");
    let replacement = format!("{key}={value}");

    let mut found = false;
    let mut lines = Vec::new();

    for line in content.lines() {
        if line.starts_with(&key_prefix) || line == not_set_line {
            if !found {
                lines.push(replacement.clone());
                found = true;
            }
            continue;
        }

        lines.push(line.to_string());
    }

    if !found {
        lines.push(replacement);
    }

    lines.join("\n") + "\n"
}

fn truncate_to_len(input: &str, max_len: usize) -> String {
    input.chars().take(max_len).collect()
}

fn build_sm8750_localversion(base: &str, short_sha: &str, kernel_version: &str) -> Result<String> {
    const UNAME_MAX_VISIBLE_LEN: usize = 63;

    let normalized_base = if base.trim().is_empty() {
        "-Kokuban".to_string()
    } else {
        format!("-{}", base.trim().trim_start_matches('-'))
    };
    let commit_suffix = format!("-g{}", short_sha);

    if kernel_version.len() >= UNAME_MAX_VISIBLE_LEN {
        return Err(anyhow!(
            "kernelversion is too long for sm8750 uname limit: {}",
            kernel_version
        ));
    }

    let max_localversion_len = UNAME_MAX_VISIBLE_LEN.saturating_sub(kernel_version.len());
    if commit_suffix.len() > max_localversion_len {
        return Err(anyhow!(
            "Not enough uname budget for sm8750 localversion suffix {}",
            commit_suffix
        ));
    }

    let max_base_len = max_localversion_len - commit_suffix.len();
    let truncated_base = truncate_to_len(&normalized_base, max_base_len);

    Ok(format!("{}{}", truncated_base, commit_suffix))
}

fn find_first_existing_path(base: &Path, candidates: &[String]) -> Option<PathBuf> {
    candidates
        .iter()
        .map(|candidate| base.join(candidate))
        .find(|path| path.exists())
}

fn find_first_existing_dir(base: &Path, candidates: &[String]) -> Option<PathBuf> {
    candidates
        .iter()
        .map(|candidate| base.join(candidate))
        .find(|path| path.is_dir())
}

fn ak3_bool_flag(value: bool) -> &'static str {
    if value { "1" } else { "0" }
}

fn ak3_action_comment(action: &str) -> &'static str {
    match action {
        "split_boot" => {
            "use split_boot to skip ramdisk unpack, e.g. for devices with init_boot ramdisk"
        }
        "dump_boot" => {
            "unpack ramdisk since it is the new first stage init ramdisk where overlay.d must go"
        }
        "flash_boot" => {
            "use flash_boot to skip ramdisk repack, e.g. for devices with init_boot ramdisk"
        }
        "write_boot" => "use write_boot to repack ramdisk, e.g. for devices with init_boot ramdisk",
        _ => "",
    }
}

fn replace_between_markers(
    content: &str,
    start_marker: &str,
    end_marker: &str,
    replacement: &str,
) -> Result<String> {
    let start = content
        .find(start_marker)
        .ok_or_else(|| anyhow!("Marker not found in anykernel.sh: {}", start_marker))?
        + start_marker.len();
    let end = content[start..]
        .find(end_marker)
        .ok_or_else(|| anyhow!("Marker not found in anykernel.sh: {}", end_marker))?
        + start;

    Ok(format!(
        "{}{}{}",
        &content[..start],
        replacement,
        &content[end..]
    ))
}

fn replace_line_with_prefix(content: &str, prefix: &str, replacement: &str) -> Result<String> {
    let mut found = false;
    let mut lines = Vec::new();

    for line in content.lines() {
        if !found && line.starts_with(prefix) {
            lines.push(replacement.to_string());
            found = true;
        } else {
            lines.push(line.to_string());
        }
    }

    if !found {
        return Err(anyhow!("Line prefix not found in anykernel.sh: {}", prefix));
    }

    Ok(lines.join("\n") + "\n")
}

fn render_anykernel_properties(config: &AnyKernelConfig) -> String {
    let mut lines = vec![
        format!("kernel.string={}", config.kernel_string),
        format!("do.devicecheck={}", ak3_bool_flag(config.device_check)),
        format!("do.modules={}", ak3_bool_flag(config.modules)),
        format!("do.systemless={}", ak3_bool_flag(config.systemless)),
        format!("do.cleanup={}", ak3_bool_flag(config.cleanup)),
        format!(
            "do.cleanuponabort={}",
            ak3_bool_flag(config.cleanup_on_abort)
        ),
    ];

    for (idx, device_name) in config.device_names.iter().enumerate() {
        lines.push(format!("device.name{}={}", idx + 1, device_name));
    }

    lines.push(format!(
        "supported.versions={}",
        config.supported_versions.as_deref().unwrap_or("")
    ));
    lines.push(format!(
        "supported.patchlevels={}",
        config.supported_patchlevels.as_deref().unwrap_or("")
    ));
    lines.push(format!(
        "supported.vendorpatchlevels={}",
        config.supported_vendorpatchlevels.as_deref().unwrap_or("")
    ));

    lines.join("\n") + "\n"
}

fn render_anykernel_boot_section_body(config: &AnyKernelConfig) -> String {
    let mut lines = Vec::new();
    if let Some(action) = config.boot_setup.as_deref() {
        lines.push(format!("{}; # {}", action, ak3_action_comment(action)));
    }

    lines.push(String::new());

    if let Some(action) = config.boot_finalize.as_deref() {
        lines.push(format!("{}; # {}", action, ak3_action_comment(action)));
    }

    lines.join("\n")
}

fn apply_anykernel_config(anykernel_dir: &Path, config: &AnyKernelConfig) -> Result<()> {
    let anykernel_path = anykernel_dir.join("anykernel.sh");
    let mut anykernel_sh = fs::read_to_string(&anykernel_path)?;

    anykernel_sh = replace_between_markers(
        &anykernel_sh,
        "properties() { '\n",
        "'; } # end properties",
        &render_anykernel_properties(config),
    )?;
    anykernel_sh =
        replace_line_with_prefix(&anykernel_sh, "BLOCK=", &format!("BLOCK={};", config.block))?;
    anykernel_sh = replace_line_with_prefix(
        &anykernel_sh,
        "IS_SLOT_DEVICE=",
        &format!("IS_SLOT_DEVICE={};", ak3_bool_flag(config.is_slot_device)),
    )?;
    anykernel_sh = replace_line_with_prefix(
        &anykernel_sh,
        "RAMDISK_COMPRESSION=",
        &format!(
            "RAMDISK_COMPRESSION={};",
            config.ramdisk_compression.as_deref().unwrap_or("auto")
        ),
    )?;
    anykernel_sh = replace_line_with_prefix(
        &anykernel_sh,
        "PATCH_VBMETA_FLAG=",
        &format!(
            "PATCH_VBMETA_FLAG={};",
            config.patch_vbmeta_flag.as_deref().unwrap_or("auto")
        ),
    )?;
    anykernel_sh = replace_between_markers(
        &anykernel_sh,
        "# boot install\n",
        "## end boot install",
        &format!("{}\n", render_anykernel_boot_section_body(config)),
    )?;

    fs::write(anykernel_path, anykernel_sh)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn applies_anykernel_config_to_upstream_template() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let temp_dir = std::env::temp_dir().join(format!("ak3-config-test-{unique}"));
        fs::create_dir_all(&temp_dir).unwrap();

        let template = r#"properties() { '
kernel.string=ExampleKernel by osm0sis @ xda-developers
do.devicecheck=1
do.modules=0
do.systemless=1
do.cleanup=1
do.cleanuponabort=0
device.name1=maguro
supported.versions=
supported.patchlevels=
supported.vendorpatchlevels=
'; } # end properties

# boot shell variables
BLOCK=/dev/block/platform/omap/omap_hsmmc.0/by-name/boot;
IS_SLOT_DEVICE=0;
RAMDISK_COMPRESSION=auto;
PATCH_VBMETA_FLAG=auto;

# boot install
dump_boot; # use split_boot to skip ramdisk unpack, e.g. for devices with init_boot ramdisk
backup_file init.rc;
write_boot; # use flash_boot to skip ramdisk repack, e.g. for devices with init_boot ramdisk
## end boot install
"#;
        fs::write(temp_dir.join("anykernel.sh"), template).unwrap();

        let config = AnyKernelConfig {
            kernel_string: "S23-Knox-Disabled-Kernel-Kokuban".to_string(),
            device_check: true,
            modules: false,
            systemless: true,
            cleanup: true,
            cleanup_on_abort: false,
            device_names: vec!["dm3q".to_string(), "dm2q".to_string(), "dm1q".to_string()],
            supported_versions: Some(String::new()),
            supported_patchlevels: Some(String::new()),
            supported_vendorpatchlevels: Some(String::new()),
            block: "/dev/block/by-name/boot".to_string(),
            is_slot_device: false,
            ramdisk_compression: Some("auto".to_string()),
            patch_vbmeta_flag: Some("auto".to_string()),
            boot_setup: Some("split_boot".to_string()),
            boot_finalize: Some("flash_boot".to_string()),
        };

        apply_anykernel_config(&temp_dir, &config).unwrap();
        let rendered = fs::read_to_string(temp_dir.join("anykernel.sh")).unwrap();

        assert!(rendered.contains("kernel.string=S23-Knox-Disabled-Kernel-Kokuban"));
        assert!(rendered.contains("device.name1=dm3q"));
        assert!(rendered.contains("device.name3=dm1q"));
        assert!(rendered.contains("BLOCK=/dev/block/by-name/boot;"));
        assert!(rendered.contains("split_boot; # use split_boot to skip ramdisk unpack"));
        assert!(rendered.contains("flash_boot; # use flash_boot to skip ramdisk repack"));
        assert!(!rendered.contains("backup_file init.rc;"));
        assert!(!rendered.contains("ExampleKernel by osm0sis"));

        fs::remove_dir_all(&temp_dir).unwrap();
    }
}

fn copy_dir_files(source: &Path, dest: &Path) -> Result<()> {
    if !source.is_dir() {
        return Err(anyhow!("Source directory not found: {:?}", source));
    }

    fs::create_dir_all(dest)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_file() {
            let file_name = path
                .file_name()
                .ok_or_else(|| anyhow!("Invalid file path: {:?}", path))?;
            fs::copy(&path, dest.join(file_name))?;
        }
    }

    Ok(())
}

fn stage_patch_in_cwd(patch_file: &Path, cwd: &Path) -> Result<PathBuf> {
    let file_name = patch_file
        .file_name()
        .ok_or_else(|| anyhow!("Invalid patch file path: {:?}", patch_file))?;
    let staged_patch = cwd.join(file_name);
    fs::copy(patch_file, &staged_patch)?;
    Ok(staged_patch)
}

fn cleanup_staged_patch(staged_patch: &Path, original_patch: &Path) {
    if staged_patch != original_patch {
        let _ = fs::remove_file(staged_patch);
    }
}

fn run_patch_command(patch_file: &Path, cwd: &Path, dry_run: bool) -> Result<bool> {
    let staged_patch = stage_patch_in_cwd(patch_file, cwd)?;
    let result = (|| {
        let patch_name = staged_patch
            .file_name()
            .ok_or_else(|| anyhow!("Invalid staged patch path: {:?}", staged_patch))?;

        let mut command = std::process::Command::new("patch");
        command.arg("-p1").arg("-N").arg("-F").arg("3");
        if dry_run {
            command.arg("--dry-run");
        }
        let status = command
            .arg("-i")
            .arg(patch_name)
            .current_dir(cwd)
            .status()?;
        Ok(status.success())
    })();
    cleanup_staged_patch(&staged_patch, patch_file);
    result
}

fn can_apply_patch(patch_file: &Path, cwd: &Path) -> Result<bool> {
    run_patch_command(patch_file, cwd, true)
}

fn run_patch(patch_file: &Path, cwd: &Path) -> Result<bool> {
    run_patch_command(patch_file, cwd, false)
}

fn apply_patch_once(patch_file: &Path, cwd: &Path) -> Result<bool> {
    if can_apply_patch(patch_file, cwd)? {
        return run_patch(patch_file, cwd);
    }
    Ok(false)
}

fn apply_patch_with_fallbacks(
    patch_file: &Path,
    kernel_source_path: &Path,
    fallback_dirs: &[String],
) -> Result<()> {
    if apply_patch_once(patch_file, kernel_source_path)? {
        return Ok(());
    }

    for fallback in fallback_dirs {
        let cwd = kernel_source_path.join(fallback);
        if cwd.is_dir() && apply_patch_once(patch_file, &cwd)? {
            return Ok(());
        }
    }

    Err(anyhow!("Failed to apply patch {:?}", patch_file))
}

fn ensure_bbg_lsm(content: &str) -> String {
    let mut lines = Vec::new();
    let mut in_lsm_block = false;

    for line in content.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("config LSM") {
            in_lsm_block = true;
        } else if in_lsm_block
            && !trimmed.is_empty()
            && !line.starts_with(' ')
            && !line.starts_with('\t')
            && !trimmed.starts_with("help")
        {
            in_lsm_block = false;
        }

        if in_lsm_block && trimmed.starts_with("default") && line.contains("selinux") {
            if line.contains("baseband_guard") {
                lines.push(line.to_string());
            } else {
                lines.push(line.replacen("selinux", "selinux,baseband_guard", 1));
            }
            continue;
        }

        lines.push(line.to_string());
    }

    lines.join("\n") + "\n"
}

fn apply_susfs_overlay(kernel_source_path: &Path, susfs: &SusfsConfig) -> Result<()> {
    let temp_dir = kernel_source_path.join(".susfs_workspace");
    if temp_dir.exists() {
        fs::remove_dir_all(&temp_dir)?;
    }

    run_cmd(
        &[
            "git",
            "clone",
            "--depth=1",
            "--branch",
            &susfs.branch,
            &susfs.repo,
            temp_dir
                .to_str()
                .ok_or_else(|| anyhow!("Invalid temp path"))?,
        ],
        None,
        false,
    )?;

    let patch_path = temp_dir.join(&susfs.patch_path);
    if !patch_path.exists() {
        fs::remove_dir_all(&temp_dir)?;
        return Err(anyhow!("SuSFS patch not found: {:?}", patch_path));
    }

    let fs_source = temp_dir.join(susfs.fs_patch_dir.as_deref().unwrap_or("kernel_patches/fs"));
    let fs_target = find_first_existing_dir(
        kernel_source_path,
        &[
            "common/fs".to_string(),
            "kernel_platform/common/fs".to_string(),
            "fs".to_string(),
        ],
    )
    .ok_or_else(|| anyhow!("Could not locate kernel fs directory for SuSFS"))?;
    copy_dir_files(&fs_source, &fs_target)?;

    let include_source = temp_dir.join(
        susfs
            .include_linux_patch_dir
            .as_deref()
            .unwrap_or("kernel_patches/include/linux"),
    );
    let include_target = find_first_existing_dir(
        kernel_source_path,
        &[
            "common/include/linux".to_string(),
            "kernel_platform/common/include/linux".to_string(),
            "include/linux".to_string(),
        ],
    )
    .ok_or_else(|| anyhow!("Could not locate kernel include/linux directory for SuSFS"))?;
    copy_dir_files(&include_source, &include_target)?;

    apply_patch_with_fallbacks(
        &patch_path,
        kernel_source_path,
        &["common".to_string(), "kernel_platform/common".to_string()],
    )?;

    fs::remove_dir_all(&temp_dir)?;
    Ok(())
}

fn apply_bbg_overlay(
    kernel_source_path: &Path,
    proj: &ProjectConfig,
    bbg: Option<&BbgConfig>,
) -> Result<()> {
    let defconfig_path = find_first_existing_path(
        kernel_source_path,
        &[
            format!("arch/arm64/configs/{}", proj.defconfig),
            format!("common/arch/arm64/configs/{}", proj.defconfig),
            format!("kernel_platform/arch/arm64/configs/{}", proj.defconfig),
            format!(
                "kernel_platform/common/arch/arm64/configs/{}",
                proj.defconfig
            ),
        ],
    )
    .ok_or_else(|| anyhow!("Could not locate defconfig for BBG"))?;
    let defconfig_content = fs::read_to_string(&defconfig_path).unwrap_or_default();
    fs::write(
        &defconfig_path,
        upsert_kconfig_entry(&defconfig_content, "CONFIG_BBG", "y"),
    )?;

    let common_root = find_first_existing_dir(
        kernel_source_path,
        &[
            "common".to_string(),
            "kernel_platform/common".to_string(),
            ".".to_string(),
        ],
    )
    .ok_or_else(|| anyhow!("Could not locate common source root for BBG"))?;

    let setup_url = bbg
        .and_then(|cfg| cfg.setup_url.as_deref())
        .unwrap_or("https://github.com/vc-teahouse/Baseband-guard/raw/main/setup.sh");
    let cmd = format!("curl -LSs '{}' | bash", setup_url);
    run_cmd(&["bash", "-c", &cmd], Some(&common_root), false)?;

    let security_kconfig = find_first_existing_path(
        &common_root,
        &[
            "security/Kconfig".to_string(),
            "../security/Kconfig".to_string(),
        ],
    )
    .or_else(|| {
        find_first_existing_path(
            kernel_source_path,
            &[
                "common/security/Kconfig".to_string(),
                "kernel_platform/common/security/Kconfig".to_string(),
                "security/Kconfig".to_string(),
            ],
        )
    })
    .ok_or_else(|| anyhow!("Could not locate security/Kconfig for BBG"))?;

    let security_content = fs::read_to_string(&security_kconfig).unwrap_or_default();
    fs::write(&security_kconfig, ensure_bbg_lsm(&security_content))?;

    Ok(())
}

fn patch_setlocalversion_remove_dirty(kernel_source_path: &Path) -> Result<()> {
    let setlocalversion_path = kernel_source_path.join("scripts/setlocalversion");
    if setlocalversion_path.exists() {
        let mut content = fs::read_to_string(&setlocalversion_path).unwrap_or_default();
        content = content.replace(" -dirty", "");

        let dirty_cleanup_line = r#"res=$(echo "$res" | sed 's/-dirty//g')"#;
        let final_release_echo = r#"echo "${KERNELVERSION}${file_localversion}${config_localversion}${LOCALVERSION}${scm_version}""#;

        if !content.contains(dirty_cleanup_line) {
            if let Some(final_echo_pos) = content.rfind(final_release_echo) {
                content.insert_str(final_echo_pos, &format!("{dirty_cleanup_line}\n"));
            } else {
                if !content.ends_with('\n') {
                    content.push('\n');
                }
                content.push_str(dirty_cleanup_line);
                content.push('\n');
            }
        }

        fs::write(&setlocalversion_path, content)?;
    }

    Ok(())
}

fn apply_sm8850_localversion(
    kernel_source_path: &Path,
    defconfig_name: &str,
    localversion: &str,
) -> Result<()> {
    patch_setlocalversion_remove_dirty(kernel_source_path)?;

    let setlocalversion_path = kernel_source_path.join("scripts/setlocalversion");
    if setlocalversion_path.exists() {
        let mut content = fs::read_to_string(&setlocalversion_path).unwrap_or_default();

        content = content.replace("${scm_version}", "");
        fs::write(&setlocalversion_path, content)?;
    }

    let defconfig_path = kernel_source_path.join(format!("arch/arm64/configs/{}", defconfig_name));
    if defconfig_path.exists() {
        let mut defconfig_content = fs::read_to_string(&defconfig_path).unwrap_or_default();
        defconfig_content = upsert_kconfig_entry(
            &defconfig_content,
            "CONFIG_LOCALVERSION",
            &format!("\"{}\"", localversion),
        );
        defconfig_content =
            upsert_kconfig_entry(&defconfig_content, "CONFIG_LOCALVERSION_AUTO", "n");
        fs::write(defconfig_path, defconfig_content)?;
    }

    Ok(())
}

fn uses_file_localversion(proj: &ProjectConfig) -> bool {
    proj.version_method.as_deref().unwrap_or("param") == "file"
}

fn run_make_targets(
    kernel_source_path: &Path,
    build_env: &HashMap<String, String>,
    make_args: &[&str],
    targets: &[&str],
    source_setup_env: bool,
) -> Result<()> {
    if source_setup_env {
        let mut cmd_str = "source ./_setup_env.sh 2>/dev/null || true && make".to_string();
        for target in targets {
            cmd_str.push(' ');
            cmd_str.push_str(target);
        }
        for arg in make_args {
            cmd_str.push_str(&format!(" '{}'", arg));
        }
        run_cmd_with_env(
            &["bash", "-c", &cmd_str],
            Some(kernel_source_path),
            build_env,
        )
    } else {
        let mut cmd = vec!["make"];
        cmd.extend_from_slice(make_args);
        cmd.extend_from_slice(targets);
        run_cmd_with_env(&cmd, Some(kernel_source_path), build_env)
    }
}

fn capture_make_output(
    kernel_source_path: &Path,
    target: &str,
    source_setup_env: bool,
) -> Result<String> {
    let output = if source_setup_env {
        let cmd = format!(
            "source ./_setup_env.sh 2>/dev/null || true && make {}",
            target
        );
        run_cmd(&["bash", "-c", &cmd], Some(kernel_source_path), true)?
    } else {
        run_cmd(&["make", target], Some(kernel_source_path), true)?
    };

    Ok(output
        .unwrap_or_else(|| "unknown".to_string())
        .trim()
        .to_string())
}

fn update_kconfig_file(path: &Path, entries: &[(&str, &str)]) -> Result<()> {
    if !path.exists() {
        return Ok(());
    }

    let mut content = fs::read_to_string(path).unwrap_or_default();
    for (key, value) in entries {
        content = upsert_kconfig_entry(&content, key, value);
    }
    fs::write(path, content)?;
    Ok(())
}

fn prepare_sm8850_build(
    kernel_source_path: &Path,
    proj: &ProjectConfig,
    enable_ksu: bool,
) -> Result<()> {
    let build_config_path = kernel_source_path.join("build.config.gki");
    if build_config_path.exists() {
        let content = fs::read_to_string(&build_config_path).unwrap_or_default();
        fs::write(&build_config_path, content.replace("check_defconfig", ""))?;
    }

    let defconfig_file = kernel_source_path.join(format!("arch/arm64/configs/{}", proj.defconfig));
    let mut entries = vec![
        ("CONFIG_RUST", "y"),
        ("CONFIG_ANDROID_BINDER_IPC_RUST", "m"),
        ("CONFIG_CC_OPTIMIZE_FOR_PERFORMANCE", "y"),
        ("CONFIG_HEADERS_INSTALL", "n"),
        ("CONFIG_TMPFS_XATTR", "y"),
        ("CONFIG_TMPFS_POSIX_ACL", "y"),
    ];
    if enable_ksu {
        entries.push(("CONFIG_KSU", "y"));
    }
    update_kconfig_file(&defconfig_file, &entries)
}

/// Merge a NetHunter defconfig fragment on top of the freshly-generated
/// out/.config. gki_defconfig itself is never modified so the frozen GKI KMI
/// baseline stays intact; a later `olddefconfig` resolves the merged result.
fn apply_nethunter_fragment(
    kernel_source_path: &Path,
    fragment_name: &str,
    build_env: &HashMap<String, String>,
) -> Result<()> {
    let fragment_rel = format!("arch/arm64/configs/{}", fragment_name);
    if !kernel_source_path.join(&fragment_rel).exists() {
        return Err(anyhow!("NetHunter fragment not found: {}", fragment_rel));
    }
    println!("NetHunter: merging config fragment {}", fragment_rel);
    // -m keeps it a pure text merge (no make); build.rs runs olddefconfig after.
    run_cmd_with_env(
        &[
            "bash",
            "scripts/kconfig/merge_config.sh",
            "-m",
            "-O",
            "out",
            "out/.config",
            &fragment_rel,
        ],
        Some(kernel_source_path),
        build_env,
    )
}

fn collect_nethunter_ko(
    dir: &Path,
    wanted: &HashSet<String>,
    dest: &Path,
    all_built: &mut Vec<String>,
    packaged: &mut Vec<String>,
) -> Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        // file_type() does NOT follow symlinks: skip the "build"/"source"
        // symlinks modules_install drops in lib/modules/<rel>/ (they point back
        // into the kernel tree and would cause an infinite recursion loop).
        let file_type = entry.file_type()?;
        if file_type.is_symlink() {
            continue;
        }
        if file_type.is_dir() {
            collect_nethunter_ko(&path, wanted, dest, all_built, packaged)?;
        } else if path.extension().and_then(|e| e.to_str()) == Some("ko") {
            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                all_built.push(name.to_string());
                if wanted.contains(name) {
                    fs::copy(&path, dest.join(name))?;
                    packaged.push(name.to_string());
                }
            }
        }
    }
    Ok(())
}

/// modules_install to a staging dir, then copy ONLY whitelisted NetHunter
/// modules into the AnyKernel3 systemless payload. Stock vendor/GKI modules are
/// never shipped, so on-device MODVERSIONS stays consistent.
fn package_nethunter_modules(
    kernel_source_path: &Path,
    anykernel_dir: &Path,
    modules_list_name: &str,
    make_args: &[&str],
    build_env: &HashMap<String, String>,
    source_setup_env: bool,
) -> Result<()> {
    // INSTALL_MOD_PATH must be ABSOLUTE: with O=out the modules_install sub-make
    // runs from kernel_source/out, so a relative path would resolve to
    // kernel_source/out/<rel> (double "out") and the packaged tree would be empty.
    let staging = kernel_source_path.join("out/mod_staging");
    if staging.exists() {
        let _ = fs::remove_dir_all(&staging);
    }
    fs::create_dir_all(&staging)?;
    let abs_staging = env::current_dir()?.join(&staging);
    let mod_path_arg = format!("INSTALL_MOD_PATH={}", abs_staging.display());

    let mut mi_args: Vec<&str> = make_args.to_vec();
    mi_args.push(&mod_path_arg);
    mi_args.push("INSTALL_MOD_STRIP=1");
    mi_args.push("DEPMOD=true");
    run_make_targets(
        kernel_source_path,
        build_env,
        &mi_args,
        &["modules_install"],
        source_setup_env,
    )?;

    let list_path = kernel_source_path.join(format!("arch/arm64/configs/{}", modules_list_name));
    let list_content = fs::read_to_string(&list_path)
        .map_err(|e| anyhow!("NetHunter module list {:?} unreadable: {}", list_path, e))?;
    let wanted: HashSet<String> = list_content
        .lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(|l| format!("{}.ko", l))
        .collect();

    let dest = anykernel_dir.join("modules/system/lib/modules");
    fs::create_dir_all(&dest)?;

    let mut all_built = Vec::new();
    let mut packaged = Vec::new();
    let modules_root = staging.join("lib/modules");
    if modules_root.exists() {
        collect_nethunter_ko(&modules_root, &wanted, &dest, &mut all_built, &mut packaged)?;
    } else {
        collect_nethunter_ko(&staging, &wanted, &dest, &mut all_built, &mut packaged)?;
    }

    all_built.sort();
    let _ = fs::write(
        kernel_source_path.join("out/nethunter_built_modules.txt"),
        all_built.join("\n"),
    );
    println!(
        "NetHunter: packaged {} of {} whitelisted modules ({} built total)",
        packaged.len(),
        wanted.len(),
        all_built.len()
    );
    for m in &wanted {
        if !packaged.contains(m) {
            println!("NetHunter: NOTE whitelisted module not built/found: {}", m);
        }
    }
    Ok(())
}

/// Assemble a standalone, manager-agnostic KernelSU/Magisk module zip carrying the
/// NetHunter out-of-tree `.ko`. AnyKernel3's own `do_modules()` cannot deliver these
/// on a built-in-KSU kernel: it requires a `kernelsu_patched` marker that only exists
/// when AnyKernel itself patches KSU into boot, it hardcodes the `me.weishu.kernelsu`
/// package (ReSukiSU / KSU-Next / SukiSU-Ultra managers use randomized package names),
/// and it gates on `/data/data/android`, which enforcing SELinux blocks in the flash
/// context. So we ship the modules as a separate zip the user installs via ANY
/// KernelSU-family or Magisk manager. Reuses the `.ko` already collected into
/// `AnyKernel3/modules/system/lib/modules` by `package_nethunter_modules`.
/// Returns `Ok(true)` if a zip was produced.
fn build_oot_module_zip(module_zip_name: &str, version_str: &str) -> Result<bool> {
    let cwd = env::current_dir()?;
    let ko_src = cwd.join("AnyKernel3/modules/system/lib/modules");
    if !ko_src.exists() {
        return Ok(false);
    }
    let ko_files: Vec<_> = fs::read_dir(&ko_src)?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("ko"))
        .collect();
    if ko_files.is_empty() {
        return Ok(false);
    }

    let stage = cwd.join("oot_module_stage");
    if stage.exists() {
        let _ = fs::remove_dir_all(&stage);
    }
    let modroot = stage.join("system/lib/modules");
    fs::create_dir_all(&modroot)?;
    fs::create_dir_all(stage.join("META-INF/com/google/android"))?;
    for ko in &ko_files {
        if let Some(name) = ko.file_name() {
            fs::copy(ko, modroot.join(name))?;
        }
    }

    fs::write(
        stage.join("module.prop"),
        format!(
            "id=nethunter-oot-modules\nname=NetHunter OOT Kernel Modules\nversion={version_str}\nversionCode=1\nauthor=Picters\ndescription=Out-of-tree kernel modules (Wi-Fi injection adapters, BT, CAN, SDR/DVB, NTFS) for the matching NetHunter kernel. Install via any KernelSU-family or Magisk manager.\n"
        ),
    )?;
    fs::write(
        stage.join("customize.sh"),
        "#!/sbin/sh\nSKIPUNZIP=0\nui_print \"- Picters NetHunter OOT modules\"\nKO=$(ls \"$MODPATH/system/lib/modules/\"*.ko 2>/dev/null | wc -l)\nui_print \"- $KO drivers staged\"\nset_perm_recursive \"$MODPATH\" 0 0 0755 0644\nfor s in service.sh inject.sh action.sh nh-modules.sh; do [ -f \"$MODPATH/$s\" ] && chmod 0755 \"$MODPATH/$s\"; done\nui_print \"- Non-Wi-Fi drivers auto-load at boot\"\nui_print \"- Wi-Fi injection: module Action / Picters Manager WebUI\"\n",
    )?;
    fs::write(
        stage.join("META-INF/com/google/android/updater-script"),
        "#MAGISK\n",
    )?;
    fs::write(
        stage.join("META-INF/com/google/android/update-binary"),
        "#!/sbin/sh\numask 022\nOUTFD=$2\nZIPFILE=$3\nmount /data 2>/dev/null\nui_print() { echo \"$1\"; }\nif [ -f /data/adb/magisk/util_functions.sh ]; then\n  . /data/adb/magisk/util_functions.sh\n  install_module\n  exit 0\nfi\nui_print \"*** Install this zip from your KernelSU/Magisk manager (Modules > Install from storage) ***\"\nexit 1\n",
    )?;

    // Picters Manager WebUI (KernelSU/SukiSU/ReSukiSU `webroot`) — one-tap cfg80211
    // vendor<->kernel toggle — plus a shell Action fallback for managers without WebUI.
    fs::create_dir_all(stage.join("webroot"))?;
    fs::write(
        stage.join("webroot/index.html"),
        include_str!("picters_webui.html"),
    )?;
    // Boot auto-load of the non-Wi-Fi drivers (CAN/BT/SDR/serial/NTFS/USB-eth) via a
    // KernelSU/Magisk service.sh, plus the injection toggle scripts + Action fallback.
    fs::write(stage.join("nh-modules.sh"), include_str!("mod_nh_modules.sh"))?;
    fs::write(stage.join("service.sh"), include_str!("mod_service.sh"))?;
    fs::write(stage.join("inject.sh"), include_str!("mod_inject.sh"))?;
    fs::write(stage.join("action.sh"), include_str!("mod_action.sh"))?;

    let out_zip = cwd.join(module_zip_name);
    if out_zip.exists() {
        let _ = fs::remove_file(&out_zip);
    }
    run_cmd(
        &["zip", "-r9", out_zip.to_str().unwrap_or(module_zip_name), "."],
        Some(stage.as_path()),
        false,
    )?;
    let _ = fs::remove_dir_all(&stage);
    Ok(out_zip.exists())
}

/// Print the module_layout CRC so a KMI regression is caught in the build log
/// WITHOUT flashing. Stock booted baseline is 0xe976b219; any change means
/// stock vendor modules would fail MODVERSIONS -> bootloop.
fn dump_kmi_baseline(kernel_source_path: &Path) {
    let candidates = ["out/Module.symvers", "out/vmlinux.symvers"];
    for cand in candidates {
        let symvers = kernel_source_path.join(cand);
        if !symvers.exists() {
            continue;
        }
        let content = fs::read_to_string(&symvers).unwrap_or_default();
        for line in content.lines() {
            let mut fields = line.split('\t');
            let crc = fields.next().unwrap_or("");
            let sym = fields.next().unwrap_or("");
            if sym == "module_layout" {
                println!(
                    "KMI-CHECK [{}] module_layout CRC = {} (stock baseline 0xe976b219)",
                    cand, crc
                );
                if crc.eq_ignore_ascii_case("0xe976b219") {
                    println!("KMI-CHECK: PASS - module_layout matches stock, KMI preserved.");
                } else {
                    println!(
                        "KMI-CHECK: WARNING - module_layout differs from stock! Vendor modules may fail MODVERSIONS; investigate before flashing."
                    );
                }
                return;
            }
        }
    }
    println!("KMI-CHECK: module_layout symbol not found in symvers; cannot verify KMI.");
}

/// Recursively collect *.c / *.h files under `dir`.
fn collect_c_sources(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_c_sources(&path, out);
        } else if matches!(
            path.extension().and_then(|e| e.to_str()),
            Some("c") | Some("h")
        ) {
            out.push(path);
        }
    }
}

/// Realtek OOT USB Wi-Fi drivers (aircrack-ng / morrownr forks) register their
/// tasklet callbacks through function-pointer casts that hide a prototype
/// mismatch: the callbacks are declared `(void *priv)` but the kernel invokes
/// them as `void(unsigned long)` (tasklet `->func`). On GKI kernels built with
/// CONFIG_CFI_CLANG the indirect call in `tasklet_action_common` validates the
/// callback's *real* prototype and hard-panics ("CFI: Fatal exception in
/// interrupt") on the mismatch — an instant reboot the moment the adapter's
/// netdev is brought up (TX schedules the xmit tasklet, RX the recv tasklet).
/// Rewrite the prototypes to the exact type the kernel calls them with; the cast
/// at the registration site then becomes a harmless no-op. Whole-tree, literal,
/// idempotent — a no-op on already-patched or non-Realtek sources. Diagnosed
/// on-device via last_kmsg CFI trace (rtl8822bu_xmit_tasklet, type 0xb990318e).
///
/// NOTE: the URB-completion callbacks (`usb_*_complete`,
/// `_usbctrl_vendorreq_async_callback`) look similarly mis-typed (they carry a
/// vestigial `struct pt_regs *regs` arg) but MUST NOT be touched: the drivers
/// already strip it via function-like compat macros in `usb_ops_linux.h`
/// (`#define usb_write_port_complete(purb, regs) usb_write_port_complete(purb)`),
/// so the compiled callbacks are already `void(struct urb *)` and CFI-safe.
/// Editing their definitions turns the 2-arg macro invocation into a 1-arg one
/// ("too few arguments provided to function-like macro invocation").
///
/// A SECOND, distinct CFI trap lives on the netdev TX path. The drivers declare
/// `int rtw_xmit_entry(_pkt *pkt, _nic_hdl pnetdev)` and assign it straight to
/// `net_device_ops.ndo_start_xmit`, which the kernel calls as
/// `netdev_tx_t(*)(struct sk_buff *, struct net_device *)`. The args match after
/// typedef, but the *return type* differs (`int` vs the `netdev_tx_t` enum), so
/// the CFI type-id differs and `dev_hard_start_xmit`'s indirect call hard-panics
/// ("CFI failure at dev_hard_start_xmit ... target: rtw_xmit_entry ...") the
/// first time a frame is actually transmitted through the netdev. This does NOT
/// fire on `iface up` or monitor scanning (no ndo TX), only once a tool pushes a
/// real frame — e.g. reaver/pixiedust's WPS EAPOL exchange via AF_PACKET
/// sendto(). Diagnosed on-device via last_kmsg (target rtw_xmit_entry [88x2bu],
/// expected type 0xc4815b0f, Comm: reaver). Rewrite the return type to
/// `netdev_tx_t` at both the definition and the `extern` decls; the `return 0;`
/// / `return ret;` bodies stay valid (NETDEV_TX_OK == 0).
///
/// The SAME return-type bug is independently hardcoded (no `#if
/// LINUX_VERSION_CODE` guard, unlike e.g. `add_virtual_intf`) in TWO more
/// netdev TX entry points that each drive their own separate `net_device_ops`
/// table, so fixing `rtw_xmit_entry` alone leaves both unpatched:
/// - `rtw_cfg80211_monitor_if_xmit_entry` — the TX entry of the *cfg80211
///   virtual monitor-mode netdev* (`rtw_cfg80211_monitor_if_ops`), i.e.
///   exactly the `wlan0mon`-style interface airmon-ng/wifite create via `iw
///   set type monitor` and that aircrack-ng-suite tools inject through.
/// - `mgnt_xmit_entry` — the TX entry of the separate "hostapd mgnt" netdev
///   (`rtl871x_mgnt_netdev_ops`) used for raw management-frame injection.
/// Both are `static`, so unlike `rtw_xmit_entry` there is no header `extern`
/// copy to patch — the definition line is the only occurrence.
fn patch_realtek_cfi(subdir: &Path) {
    let rules: [(&str, &str, &str); 6] = [
        (
            "usb_recv_tasklet(void *priv)",
            "usb_recv_tasklet(unsigned long priv)",
            "recv-tasklet",
        ),
        (
            "_xmit_tasklet(void *priv)",
            "_xmit_tasklet(unsigned long priv)",
            "xmit-tasklet",
        ),
        (
            "mpath_tx_tasklet_hdl(void *priv)",
            "mpath_tx_tasklet_hdl(unsigned long priv)",
            "mesh-tasklet",
        ),
        (
            "int rtw_xmit_entry(_pkt *pkt, _nic_hdl pnetdev)",
            "netdev_tx_t rtw_xmit_entry(_pkt *pkt, _nic_hdl pnetdev)",
            "ndo-start-xmit",
        ),
        (
            "int rtw_cfg80211_monitor_if_xmit_entry(struct sk_buff *skb, struct net_device *ndev)",
            "netdev_tx_t rtw_cfg80211_monitor_if_xmit_entry(struct sk_buff *skb, struct net_device *ndev)",
            "monitor-if-xmit",
        ),
        (
            "int mgnt_xmit_entry(struct sk_buff *skb, struct net_device *pnetdev)",
            "netdev_tx_t mgnt_xmit_entry(struct sk_buff *skb, struct net_device *pnetdev)",
            "mgnt-xmit",
        ),
    ];

    let mut files = Vec::new();
    collect_c_sources(subdir, &mut files);

    let mut hits = [0usize; 6];
    for file in files {
        let Ok(content) = fs::read_to_string(&file) else {
            continue;
        };
        let mut patched = content.clone();
        for (i, &(needle, repl, _)) in rules.iter().enumerate() {
            let n = patched.matches(needle).count();
            if n > 0 {
                hits[i] += n;
                patched = patched.replace(needle, repl);
            }
        }
        if patched != content {
            let _ = fs::write(&file, patched);
        }
    }

    for (i, &(_, _, label)) in rules.iter().enumerate() {
        if hits[i] > 0 {
            println!("NetHunter OOT CFI patch: {} x{}", label, hits[i]);
        }
    }
    if hits.iter().all(|&n| n == 0) {
        println!("NetHunter OOT CFI patch: no patterns matched (already patched / non-Realtek)");
    }
}

/// The aircrack-ng forks (rtl8812au -> 88XXau.ko, rtl8188eus -> 8188eu.ko)
/// declare their variable-length 802.11 IE struct with a fixed 1-byte
/// trailing member (`UCHAR data[1]`) instead of a C99 flexible array member,
/// then read past it unconditionally — e.g. `check_assoc_AP()` in
/// rtw_wlan_util.c does `_rtw_memcmp(pIE->data, <OUI>, 3)` (and even
/// `pIE->data[4]`/`[5]` for the Realtek vendor IE branch) against a
/// statically 1-byte array. On a GKI kernel built with UBSAN's array-bounds
/// sanitizer this is a compile-time-provable violation, not a runtime
/// buffer overrun, so it hard-panics
/// ("UBSAN: array index out of bounds ... Fatal exception in interrupt")
/// the moment the driver actually parses a real (assoc-response/beacon) IE
/// list from the air — i.e. on a normal 802.11 association, which
/// wpa_supplicant-driven tools (OneShot) trigger via cfg80211_connect but
/// raw-WPS-only tools (reaver/bully) mostly don't. Diagnosed on-device via
/// last_kmsg: `check_assoc_AP+0x1c8/0x1d0 [88XXau]`, Comm: wpa_supplicant,
/// called from rtw_joinbss_cmd -> ... -> cfg80211_connect -> nl80211_connect.
/// Fix: widen `data[1]` to `data[]` in `include/wlan_bssdef.h`
/// (NDIS_802_11_VARIABLE_IEs). Purely a type-level annotation change — the
/// struct is never used with fixed-size `sizeof()`, so this doesn't move
/// any real memory layout, it just removes the false static-bounds
/// violation while the driver's own `i < len` loop bound keeps doing the
/// actual (runtime) bounds checking it always relied on. The morrownr forks
/// (8814au, 88x2bu) already use `u8 data[]` upstream and are unaffected.
fn patch_realtek_ubsan(subdir: &Path) {
    let rules: [(&str, &str, &str); 1] = [(
        "UCHAR  data[1];",
        "UCHAR  data[];",
        "var-ie-flexarray",
    )];

    let mut files = Vec::new();
    collect_c_sources(subdir, &mut files);

    let mut hits = [0usize; 1];
    for file in files {
        let Ok(content) = fs::read_to_string(&file) else {
            continue;
        };
        let mut patched = content.clone();
        for (i, &(needle, repl, _)) in rules.iter().enumerate() {
            let n = patched.matches(needle).count();
            if n > 0 {
                hits[i] += n;
                patched = patched.replace(needle, repl);
            }
        }
        if patched != content {
            let _ = fs::write(&file, patched);
        }
    }

    for (i, &(_, _, label)) in rules.iter().enumerate() {
        if hits[i] > 0 {
            println!("NetHunter OOT UBSAN patch: {} x{}", label, hits[i]);
        }
    }
    if hits.iter().all(|&n| n == 0) {
        println!("NetHunter OOT UBSAN patch: no patterns matched (already patched / non-aircrack)");
    }
}

/// Clone + build the out-of-tree aircrack Wi-Fi injection drivers
/// (rtl8812au/8814au/8188eus for RTL8812AU/8814AU chips absent from in-tree
/// 6.12) against the just-built kernel and drop their .ko into the AnyKernel3
/// payload. BEST-EFFORT: any clone/build failure is logged and skipped so the
/// core kernel still ships. Leaf modules -> no KMI impact.
fn build_nethunter_oot_modules(
    kernel_source_path: &Path,
    anykernel_dir: &Path,
    oot: &[OotModule],
    make_args: &[&str],
    build_env: &HashMap<String, String>,
    source_setup_env: bool,
) -> Result<()> {
    let dest = anykernel_dir.join("modules/system/lib/modules");
    fs::create_dir_all(&dest)?;
    let cwd = env::current_dir()?;

    // The aircrack Makefiles append GCC-only warning flags (e.g.
    // -Wno-stringop-overread) that clang rejects under -Werror. Append
    // -Wno-unknown-warning-option (+ -Wno-error) so those are ignored, while
    // preserving the kernel's existing KCFLAGS (-D__ANDROID_COMMON_KERNEL__ ...).
    // The android16-6.12 kernel promotes specific warnings via -Werror=<name>
    // (e.g. incompatible-pointer-types) which a blanket -Wno-error does not
    // undo, so disable the exact warnings the older aircrack sources trip on.
    let oot_wflags = "-Wno-error -Wno-unknown-warning-option \
        -Wno-incompatible-pointer-types-discards-qualifiers \
        -Wno-incompatible-pointer-types -Wno-pointer-sign";
    let mut oot_env = build_env.clone();
    for key in ["KCFLAGS", "KCPPFLAGS"] {
        let base = oot_env.get(key).cloned().unwrap_or_default();
        oot_env.insert(key.to_string(), format!("{base} {oot_wflags}"));
    }

    for m in oot {
        let subdir_abs = cwd.join(kernel_source_path).join(&m.subdir);

        if !subdir_abs.exists() {
            if let Some(parent) = subdir_abs.parent() {
                let _ = fs::create_dir_all(parent);
            }
            let Some(subdir_str) = subdir_abs.to_str() else {
                println!("NetHunter OOT: invalid path for {}, skipping", m.subdir);
                continue;
            };
            let mut clone = vec!["git", "clone", "--depth=1"];
            if let Some(branch) = m.branch.as_deref() {
                clone.push("--branch");
                clone.push(branch);
            }
            clone.push(&m.repo);
            clone.push(subdir_str);
            if run_cmd(&clone, None, false).is_err() {
                println!("NetHunter OOT: clone failed for {}, skipping", m.repo);
                continue;
            }
        }

        // Some OOT Makefiles hardcode -Werror, which turns clang's stricter
        // (vs gcc) warnings into hard errors and overrides our KCFLAGS -Wno-error.
        // Strip it so the driver compiles through those warnings.
        let makefile = subdir_abs.join("Makefile");
        if let Ok(content) = fs::read_to_string(&makefile) {
            let _ = fs::write(&makefile, content.replace("-Werror", ""));
        }

        // Rewrite the driver's tasklet / URB-completion callback prototypes so
        // they match the types the kernel calls them with; otherwise the GKI
        // CONFIG_CFI_CLANG check hard-panics ("CFI: Fatal exception in
        // interrupt") the instant the adapter's interface is brought up.
        patch_realtek_cfi(&subdir_abs);

        // Widen the fixed data[1] variable-IE trailing array to a flexible
        // array member; otherwise UBSAN's array-bounds check hard-panics the
        // moment check_assoc_AP() parses a real assoc-response IE list
        // during a normal 802.11 association (e.g. wpa_supplicant-driven
        // tools like OneShot).
        patch_realtek_ubsan(&subdir_abs);

        let m_arg = format!("M={}", subdir_abs.display());
        let mut args: Vec<&str> = make_args.to_vec();
        args.push(&m_arg);
        if let Some(extra) = &m.make_args {
            for a in extra {
                args.push(a);
            }
        }

        if let Err(err) =
            run_make_targets(kernel_source_path, &oot_env, &args, &["modules"], source_setup_env)
        {
            println!(
                "NetHunter OOT: build failed for {} ({}), skipping",
                m.subdir, err
            );
            continue;
        }

        let mut shipped = 0;
        if let Ok(entries) = fs::read_dir(&subdir_abs) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) != Some("ko") {
                    continue;
                }
                let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                    continue;
                };
                let ko_path = path.to_string_lossy().to_string();
                let _ = run_cmd_with_env(
                    &["llvm-strip", "--strip-debug", ko_path.as_str()],
                    None,
                    build_env,
                );
                if fs::copy(&path, dest.join(name)).is_ok() {
                    shipped += 1;
                    println!("NetHunter OOT: packaged {}", name);
                }
            }
        }
        if shipped == 0 {
            println!("NetHunter OOT: no .ko produced by {}", m.subdir);
        }
    }
    Ok(())
}

pub fn handle_build(
    project_key: String,
    branch: String,
    do_release: bool,
    custom_localversion: Option<String>,
    resukisu_setup_arg: Option<String>,
    apply_susfs: bool,
    apply_bbg: bool,
) -> Result<()> {
    let proj = load_project(&project_key)?;

    let kernel_source_path = PathBuf::from("kernel_source");
    if !kernel_source_path.exists() {
        return Err(anyhow!("Kernel source not found at ./kernel_source"));
    }

    let target_soc_str = project_key.split('_').nth(1).unwrap_or("unknown");
    let is_sm8850 = target_soc_str == "sm8850";

    let wrapper_dir = env::current_dir()?.join(".compiler_wrappers");
    let _ = fs::create_dir_all(&wrapper_dir);

    let rust_cmd = if command_exists("sccache") {
        create_compiler_wrapper(&wrapper_dir, "rustc", "sccache", "rustc")?
    } else {
        "rustc".to_string()
    };

    let cc_cmd = if command_exists("sccache") {
        create_compiler_wrapper(&wrapper_dir, "clang", "sccache", "clang")?
    } else if command_exists("ccache") {
        create_compiler_wrapper(&wrapper_dir, "clang", "ccache", "clang")?
    } else {
        "clang".to_string()
    };

    let rustc_arg = format!("RUSTC={}", rust_cmd);
    let hostrustc_arg = format!("HOSTRUSTC={}", rust_cmd);
    let cc_arg = format!("CC={}", cc_cmd);
    let hostcc_arg = format!("HOSTCC={}", cc_cmd);

    let toolchain_prefix = proj.toolchain_path_prefix.as_deref().unwrap_or("");
    let toolchain_base = env::current_dir()?.join(toolchain_prefix);
    let offline = env_flag("KOKUBAN_OFFLINE");
    let reuse_toolchains = env_flag("KOKUBAN_REUSE_TOOLCHAINS");

    if let Some(urls) = &proj.toolchain_urls {
        let tc_download_dir = PathBuf::from("toolchain_download");
        let cache_dir = env::var_os("KOKUBAN_DOWNLOAD_CACHE_DIR").map(PathBuf::from);

        if reuse_toolchains && toolchain_paths_ready(&toolchain_base, &proj) {
            println!("Reusing existing toolchain at {}", toolchain_base.display());
        } else {
            download_toolchains(
                urls,
                &tc_download_dir,
                cache_dir.as_deref(),
                offline,
                proj.toolchain_sha256.as_ref(),
            )?;

            let extract_script = r#"
            set -e
            if ls *.tar.gz.[0-9]* 1> /dev/null 2>&1; then
                cat *.tar.gz.* | tar -zxf - --warning=no-unknown-keyword -C ..
            elif ls *part_aa* 1> /dev/null 2>&1 || ls *_aa.tar.gz 1> /dev/null 2>&1 || ls *.tar.gz.aa 1> /dev/null 2>&1; then
                cat *.tar.gz | tar -zxf - --warning=no-unknown-keyword -C ..
            else
                if ls *.tar.gz 1> /dev/null 2>&1; then
                    for tarball in *.tar.gz; do
                        tar -zxf "$tarball" --warning=no-unknown-keyword -C ..
                    done
                fi
                if ls *.tar.xz 1> /dev/null 2>&1; then
                    for tarball in *.tar.xz; do
                        tar -xf "$tarball" -C ..
                    done
                fi
                if ls *.zip 1> /dev/null 2>&1; then
                    for zipball in *.zip; do
                        unzip -o -q "$zipball" -d ..
                    done
                fi
            fi
            chmod -R +x ../bin/ 2>/dev/null || true
            chmod -R +x ../build-tools/bin/ 2>/dev/null || true
            chmod +x ../bindgen-cli-*/bindgen 2>/dev/null || true
        "#;

            run_cmd(
                &["bash", "-c", extract_script],
                Some(&tc_download_dir),
                false,
            )?;

            fs::remove_dir_all(tc_download_dir)?;
        }
    }

    let mut build_env = HashMap::new();
    let current_path = env::var("PATH").unwrap_or_default();

    let mut new_path = current_path.clone();

    if let Some(exports) = &proj.toolchain_path_exports {
        for export in exports {
            let p = toolchain_base.join(export);
            new_path = format!("{}:{}", p.display(), new_path);
        }
    } else if !toolchain_prefix.is_empty() {
        new_path = format!("{}:{}", toolchain_base.join("bin").display(), new_path);
    }

    // Ensure compat cross-linker symlinks exist for older kernels (e.g. 5.4 vdso32)
    // The kernel build uses $(CROSS_COMPILE_COMPAT)ld which resolves to arm-linux-gnueabi-ld.
    // Without this symlink, the build falls back to the system ld which lacks ARM32 support.
    if let Some(first_export) = proj.toolchain_path_exports.as_ref().and_then(|e| e.first()) {
        let bin_dir = toolchain_base.join(first_export);
        let ld_lld = bin_dir.join("ld.lld");
        if ld_lld.exists() {
            for compat_ld in &["arm-linux-gnueabi-ld", "arm-linux-gnueabi-ld.bfd"] {
                let compat_path = bin_dir.join(compat_ld);
                if !compat_path.exists() {
                    let _ = std::os::unix::fs::symlink(&ld_lld, &compat_path);
                }
            }
        }
    }

    build_env.insert("PATH".to_string(), new_path);
    build_env.insert("ARCH".to_string(), "arm64".to_string());
    build_env.insert("SUBARCH".to_string(), "arm64".to_string());
    build_env.insert("CLANG_TRIPLE".to_string(), "aarch64-linux-gnu-".to_string());
    build_env.insert(
        "CROSS_COMPILE".to_string(),
        "aarch64-linux-gnu-".to_string(),
    );
    build_env.insert(
        "CROSS_COMPILE_COMPAT".to_string(),
        "arm-linux-gnueabi-".to_string(),
    );
    build_env.insert("TZ".to_string(), "Asia/Hong_Kong".to_string());

    let mut kcflags = "-O2 -pipe -Wno-error -D__ANDROID_COMMON_KERNEL__".to_string();
    if is_sm8850 {
        if let Ok(common_real_path) = fs::canonicalize(&kernel_source_path)
            && let Some(root_real_path) = common_real_path.parent()
        {
            kcflags = format!(
                "-O2 -pipe -Wno-error -fno-stack-protector -no-canonical-prefixes -D__ANDROID_COMMON_KERNEL__ -fdebug-prefix-map={}=. -fmacro-prefix-map={}=. -ffile-prefix-map={}=.",
                root_real_path.display(),
                root_real_path.display(),
                root_real_path.display()
            );
        }
        let libclang_path = toolchain_base.join("lib");
        build_env.insert(
            "LIBCLANG_PATH".to_string(),
            libclang_path.display().to_string(),
        );
        build_env.insert("KBUILD_GENDWARFKSYMS_STABLE".to_string(), "1".to_string());
        build_env.insert("KBUILD_BUILD_USER".to_string(), "build-user".to_string());
        build_env.insert("KBUILD_BUILD_HOST".to_string(), "build-host".to_string());
        build_env.insert("LC_ALL".to_string(), "C".to_string());
    }

    build_env.insert("RUSTC".to_string(), rust_cmd.clone());
    build_env.insert("HOSTRUSTC".to_string(), rust_cmd.clone());
    build_env.insert("BINDGEN".to_string(), "bindgen".to_string());

    build_env.insert("KCFLAGS".to_string(), kcflags.clone());
    build_env.insert("KCPPFLAGS".to_string(), kcflags);
    build_env.insert("IN_KERNEL_MODULES".to_string(), "1".to_string());
    build_env.insert("DO_NOT_STRIP_MODULES".to_string(), "1".to_string());
    build_env.insert("PAGE_SIZE".to_string(), "4096".to_string());

    if let Some(true) = proj.extra_host_env {
        let kbt = toolchain_base.join("kernel-build-tools/linux-x86");
        let sysroot = toolchain_base.join("gcc/linux-x86/host/x86_64-linux-glibc2.17-4.8/sysroot");

        build_env.insert(
            "LD_LIBRARY_PATH".to_string(),
            format!(
                "{}:{}/lib64",
                env::var("LD_LIBRARY_PATH").unwrap_or_default(),
                kbt.display()
            ),
        );

        let sysroot_flag = format!("--sysroot={} ", sysroot.display());
        let cflags = format!("-I{}/include ", kbt.display());
        let ldflags = format!(
            "-L {}/lib64 -fuse-ld=lld --rtlib=compiler-rt",
            kbt.display()
        );

        build_env.insert(
            "HOSTCFLAGS".to_string(),
            format!("{}{}", sysroot_flag, cflags),
        );
        build_env.insert(
            "HOSTLDFLAGS".to_string(),
            format!("{}{}", sysroot_flag, ldflags),
        );
    }

    let resukisu_setup_arg = resukisu_setup_arg
        .as_deref()
        .map(str::trim)
        .filter(|arg| !arg.is_empty())
        .unwrap_or("main");

    let setup_url = match branch.as_str() {
        _ if is_resukisu_variant(&branch) => Some((
            "https://raw.githubusercontent.com/ReSukiSU/ReSukiSU/main/kernel/setup.sh",
            resukisu_setup_arg,
        )),
        _ => None,
    };

    if let Some((url, arg)) = setup_url {
        let cmd = format!("curl -LSs '{}' | bash -s {}", url, arg);
        run_cmd(&["bash", "-c", &cmd], Some(&kernel_source_path), false)?;
    }

    let mut feature_suffixes = Vec::new();
    if apply_susfs {
        if is_resukisu_variant(&branch) {
            let susfs = proj
                .susfs
                .as_ref()
                .ok_or_else(|| anyhow!("Project {} does not define a SuSFS source", project_key))?;
            apply_susfs_overlay(&kernel_source_path, susfs)?;
            feature_suffixes.push("susfs".to_string());
        } else {
            println!(
                "Skipping SuSFS for branch '{}': SuSFS is only enabled for ReSukiSU builds.",
                branch
            );
        }
    }

    if apply_bbg {
        apply_bbg_overlay(&kernel_source_path, &proj, proj.bbg.as_ref())?;
        feature_suffixes.push("bbg".to_string());
    }

    let kernel_version = capture_make_output(&kernel_source_path, "kernelversion", is_sm8850)?;

    let short_sha = run_cmd(
        &["git", "rev-parse", "--short=12", "HEAD"],
        Some(&kernel_source_path),
        true,
    )?
    .unwrap_or_else(|| "unknown".to_string())
    .trim()
    .to_string();

    let mut make_args = vec![
        "O=out",
        "ARCH=arm64",
        "SUBARCH=arm64",
        "LLVM=1",
        "LLVM_IAS=1",
        "LD=ld.lld",
        "HOSTLD=ld.lld",
        "AR=llvm-ar",
        "NM=llvm-nm",
        "OBJCOPY=llvm-objcopy",
        "OBJDUMP=llvm-objdump",
        "OBJSIZE=llvm-size",
        "READELF=llvm-readelf",
        "STRIP=llvm-strip",
        "BINDGEN=bindgen",
    ];

    let soc_arg = format!("TARGET_SOC={}", target_soc_str);
    make_args.push(&soc_arg);

    make_args.push(&rustc_arg);
    make_args.push(&hostrustc_arg);
    make_args.push(&cc_arg);
    make_args.push(&hostcc_arg);

    fs::write(kernel_source_path.join("protected_module_names_list"), "")?;
    fs::write(kernel_source_path.join("protected_exports_list"), "")?;

    let git_exclude_path = kernel_source_path.join(".git/info/exclude");
    let mut exclude_data = fs::read_to_string(&git_exclude_path).unwrap_or_default();
    exclude_data.push_str("\nprotected_module_names_list\nprotected_exports_list\n");
    let _ = fs::write(git_exclude_path, exclude_data);

    build_env.insert("CC".to_string(), cc_cmd.clone());
    build_env.insert("HOSTCC".to_string(), cc_cmd.clone());
    build_env.insert("LD".to_string(), "ld.lld".to_string());
    build_env.insert("HOSTLD".to_string(), "ld.lld".to_string());

    let build_variant_suffix = variant_suffix(&branch);

    let mut localversion = if let Some(ref custom) = custom_localversion {
        let custom = custom.trim();
        if is_sm8850 {
            format!("-{}", custom.trim_start_matches('-'))
        } else {
            custom.to_string()
        }
    } else {
        format!("{}-{}", proj.localversion_base, build_variant_suffix)
    };

    if target_soc_str == "sm8750" {
        let sm8750_base = custom_localversion
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or(&localversion);
        localversion = build_sm8750_localversion(sm8750_base, &short_sha, &kernel_version)?;

        println!(
            "sm8750 uname release: {}{} (len={})",
            kernel_version,
            localversion,
            kernel_version.len() + localversion.len()
        );
    }

    if is_sm8850 {
        if custom_localversion.is_none() {
            if project_key == "mi17_sm8850" {
                localversion = format!(
                    "{}-{}-g{}-4k",
                    proj.localversion_base, build_variant_suffix, short_sha
                );
            } else {
                localversion = format!("{}-g{}-4k", proj.localversion_base, short_sha);
            }
        }
        let _ = fs::write(kernel_source_path.join(".scmversion"), "");
        make_args.push("LOCALVERSION_AUTO=n");
        build_env.insert("LOCALVERSION_AUTO".to_string(), "n".to_string());
        apply_sm8850_localversion(&kernel_source_path, &proj.defconfig, &localversion)?;
    }

    if is_sm8850 {
        prepare_sm8850_build(&kernel_source_path, &proj, setup_url.is_some())?;
    }

    if is_sm8850 {
        println!("Testing Environment and rust_is_available.sh...");
        let mut cmd = std::process::Command::new("bash");
        cmd.arg("-c").arg("source ./_setup_env.sh 2>/dev/null || true && echo '=== Toolchain Versions ===' && $CC --version | head -n1 && $RUSTC -V && bindgen --version && pahole --version && echo '==========================' && sh scripts/rust_is_available.sh -v");
        cmd.current_dir(&kernel_source_path);
        for (k, v) in &build_env {
            cmd.env(k, v);
        }
        if let Ok(output) = cmd.output() {
            println!("rust_is_available.sh Exit Status: {}", output.status);
            println!("stdout:\n{}", String::from_utf8_lossy(&output.stdout));
            println!("stderr:\n{}", String::from_utf8_lossy(&output.stderr));
        } else {
            println!("Failed to execute rust_is_available.sh process.");
        }
    }

    run_make_targets(
        &kernel_source_path,
        &build_env,
        &make_args,
        &[&proj.defconfig],
        is_sm8850,
    )?;

    if let Some(fragment) = proj.nethunter_fragment.as_deref() {
        apply_nethunter_fragment(&kernel_source_path, fragment, &build_env)?;
    }

    let mut disable_configs = vec!["TRIM_UNUSED_KSYMS"];
    if let Some(disables) = &proj.disable_security {
        for d in disables {
            disable_configs.push(d);
        }
    }

    for config in disable_configs {
        run_cmd(
            &[
                "scripts/config",
                "--file",
                "out/.config",
                "--disable",
                config,
            ],
            Some(&kernel_source_path),
            false,
        )?;
    }

    if let Some(lto) = &proj.lto {
        if lto == "thin" {
            run_cmd(
                &[
                    "scripts/config",
                    "--file",
                    "out/.config",
                    "-e",
                    "LTO_CLANG_THIN",
                    "-d",
                    "LTO_CLANG_FULL",
                ],
                Some(&kernel_source_path),
                false,
            )?;
        } else if lto == "full" {
            run_cmd(
                &[
                    "scripts/config",
                    "--file",
                    "out/.config",
                    "-e",
                    "LTO_CLANG_FULL",
                    "-d",
                    "LTO_CLANG_THIN",
                ],
                Some(&kernel_source_path),
                false,
            )?;
        } else if lto == "none" {
            run_cmd(
                &[
                    "scripts/config",
                    "--file",
                    "out/.config",
                    "-e",
                    "LTO_NONE",
                    "-d",
                    "LTO_CLANG_THIN",
                    "-d",
                    "LTO_CLANG_FULL",
                ],
                Some(&kernel_source_path),
                false,
            )?;
        }
    }

    run_make_targets(
        &kernel_source_path,
        &build_env,
        &make_args,
        &["olddefconfig"],
        is_sm8850,
    )?;

    if !is_sm8850 {
        patch_setlocalversion_remove_dirty(&kernel_source_path)?;
    }

    if custom_localversion.is_some() && !is_sm8850 {
        let _ = fs::write(kernel_source_path.join(".scmversion"), "");
        make_args.push("LOCALVERSION_AUTO=n");
        build_env.insert("LOCALVERSION_AUTO".to_string(), "n".to_string());
    }

    let localversion_arg = format!("LOCALVERSION={}", localversion);

    if !is_sm8850 {
        if uses_file_localversion(&proj) {
            let _ = fs::write(
                kernel_source_path.join("localversion"),
                localversion.clone(),
            );
        } else {
            make_args.push(&localversion_arg);
            build_env.insert("LOCALVERSION".to_string(), localversion.clone());
        }
    } else {
        if uses_file_localversion(&proj) {
            let _ = fs::write(kernel_source_path.join("localversion"), "");
        }
        make_args.push("LOCALVERSION=");
        build_env.insert("LOCALVERSION".to_string(), "".to_string());
    }

    let threads = run_cmd(&["nproc"], None, true)?.unwrap().trim().to_string();
    let jobs = format!("-j{}", threads);

    if is_sm8850 {
        let image_targets = if proj.nethunter_fragment.is_some() {
            "Image modules"
        } else {
            "Image"
        };
        let mut cmd_str = format!(
            "source ./_setup_env.sh 2>/dev/null || true && make {} {}",
            jobs, image_targets
        );
        for arg in &make_args {
            cmd_str.push_str(&format!(" '{}'", arg));
        }
        run_cmd_with_env(
            &["bash", "-c", &cmd_str],
            Some(&kernel_source_path),
            &build_env,
        )?;
    } else {
        let mut build_cmd = vec!["make", &jobs, "Image", "modules"];
        build_cmd.extend_from_slice(&make_args);
        run_cmd_with_env(&build_cmd, Some(&kernel_source_path), &build_env)?;
    }

    if uses_file_localversion(&proj) {
        fs::write(kernel_source_path.join("localversion"), "")?;
    }

    prepare_anykernel_worktree(Path::new("AnyKernel3"), offline)?;

    if let Some(config_key) = proj.anykernel_config.as_deref() {
        let anykernel_config = load_anykernel_config(config_key)?;
        apply_anykernel_config(Path::new("AnyKernel3"), &anykernel_config)?;
    }

    let image_path = kernel_source_path.join("out/arch/arm64/boot/Image");
    if !image_path.exists() {
        return Err(anyhow!("Image not found at {:?}", image_path));
    }

    fs::copy(image_path, "AnyKernel3/Image")?;

    if let Some(modules_list) = proj.nethunter_modules.as_deref() {
        package_nethunter_modules(
            &kernel_source_path,
            Path::new("AnyKernel3"),
            modules_list,
            &make_args,
            &build_env,
            is_sm8850,
        )?;
    }
    if proj.nethunter_fragment.is_some() {
        dump_kmi_baseline(&kernel_source_path);
    }

    if let Some(oot) = proj.nethunter_oot_modules.as_deref() {
        build_nethunter_oot_modules(
            &kernel_source_path,
            Path::new("AnyKernel3"),
            oot,
            &make_args,
            &build_env,
            is_sm8850,
        )?;
    }

    let hkt = FixedOffset::east_opt(8 * 3600).ok_or_else(|| anyhow!("Invalid HKT offset"))?;
    let date_str = Utc::now()
        .with_timezone(&hkt)
        .format("%Y%m%d-%H%M")
        .to_string();
    let zip_prefix = proj.zip_name_prefix.as_deref().unwrap_or("Kernel");
    let feature_suffix = if feature_suffixes.is_empty() {
        String::new()
    } else {
        format!("-{}", feature_suffixes.join("-"))
    };

    let clean_localversion = localversion.trim_start_matches('-');
    let final_zip_name = format!(
        "{}-{}-{}{}-{}.zip",
        zip_prefix, kernel_version, clean_localversion, feature_suffix, date_str
    );

    run_cmd(
        &[
            "zip",
            "-r9",
            format!("../{}", final_zip_name).as_str(),
            ".",
            "-x",
            ".git*",
            "-x",
            ".github*",
            "-x",
            "README.md",
            "-x",
            "LICENSE",
            "-x",
            "*.gitignore",
            "-x",
            "patch_linux",
            "-x",
            "tools/boot.img.lz4",
            "-x",
            "tools/libmagiskboot.so",
        ],
        Some(Path::new("AnyKernel3")),
        false,
    )?;

    // Standalone, manager-agnostic OOT-modules zip (NetHunter projects only).
    let module_zip_name: Option<String> = if proj.nethunter_fragment.is_some() {
        let name = format!(
            "{}-OOT-Modules-{}{}-{}.zip",
            zip_prefix, clean_localversion, feature_suffix, date_str
        );
        let version_str = format!("{}-{}", kernel_version, clean_localversion);
        match build_oot_module_zip(&name, &version_str) {
            Ok(true) => {
                println!("NetHunter: built standalone OOT module zip {}", name);
                Some(name)
            }
            Ok(false) => {
                println!("NetHunter: no OOT .ko found, skipping module zip");
                None
            }
            Err(e) => {
                println!("NetHunter: WARN could not build OOT module zip: {}", e);
                None
            }
        }
    } else {
        None
    };

    if do_release {
        let release_tag = format!(
            "{}-{}{}-{}",
            zip_prefix, build_variant_suffix, feature_suffix, date_str
        );
        let release_title = format!(
            "{} {}{} Build ({})",
            zip_prefix, build_variant_suffix, feature_suffix, date_str
        );

        if Path::new(&final_zip_name).exists() {
            let notes = format!(
                "Automated build for {}\nKernel Version: {}",
                branch, kernel_version
            );
            let mut rel_args: Vec<&str> = vec![
                "gh",
                "release",
                "create",
                release_tag.as_str(),
                final_zip_name.as_str(),
            ];
            // Attach the standalone OOT-modules zip as a second asset when present.
            if let Some(ref mz) = module_zip_name {
                if Path::new(mz).exists() {
                    rel_args.push(mz.as_str());
                }
            }
            rel_args.push("--repo");
            rel_args.push(proj.repo.as_str());
            rel_args.push("--title");
            rel_args.push(release_title.as_str());
            rel_args.push("--notes");
            rel_args.push(notes.as_str());
            run_cmd(&rel_args, None, false)?;

            handle_notify(release_tag)?;
        } else {
            return Err(anyhow!("Final zip not found"));
        }
    }

    Ok(())
}

pub fn handle_collect_artifacts(artifact_dir: String) -> Result<()> {
    let artifact_dir = PathBuf::from(artifact_dir);
    fs::create_dir_all(&artifact_dir)?;

    let mut has_artifacts = false;

    for entry in fs::read_dir(".")? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) == Some("zip") {
            has_artifacts |= copy_artifact_if_exists(&path, &artifact_dir)?;
        }
    }

    for extra_artifact in [
        "kernel_source/out/.config",
        "kernel_source/out/vmlinux.symvers",
        "kernel_source/out/Module.symvers",
        "kernel_source/out/nethunter_built_modules.txt",
    ] {
        has_artifacts |= copy_artifact_if_exists(Path::new(extra_artifact), &artifact_dir)?;
    }

    set_github_output(
        "has_artifacts",
        if has_artifacts { "true" } else { "false" },
    )?;

    if has_artifacts {
        println!("Collected build artifacts into {}", artifact_dir.display());
    } else {
        println!("No build artifacts were produced, skipping upload.");
    }

    Ok(())
}
