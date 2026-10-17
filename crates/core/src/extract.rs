//! 解压:支持单个 .zip,以及两种原生分卷
//! - `B.z01 ... B.zip`(zip -s,末卷 .zip)→ 交给平台后端(unzip/7z)
//! - `B.zip.001 ...`(7z -v,首卷 .001)→ 只能 7z 系工具解

use std::path::Path;

use crate::backend::Backend;
use crate::Result;

/// 解压 archive 到 dest(dest 会被创建;已有同名文件被覆盖)。
/// password 与压缩时一致;无密码传 None。
pub fn extract(archive: &Path, dest: &Path, password: Option<&str>) -> Result<()> {
    // .zip.001 分卷只有 7z 系工具认识,不受平台默认后端限制
    let is_volume = archive.extension().is_some_and(|e| e.eq_ignore_ascii_case("001"));
    let backend = if is_volume {
        Backend::SevenZip
    } else {
        Backend::native()
    };
    let mut cmd = backend.extract_command(archive, dest, password);
    backend.run(&mut cmd)
}
