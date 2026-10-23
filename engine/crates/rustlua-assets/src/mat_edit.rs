//! P3-E02-2：`.material` 编辑会话（快照撤销/重做 + 严格校验 + 拒绝零副作用）。
//!
//! 会话只持有**已解析**的 [`MaterialAsset`]：读写文件由编辑器壳层负责
//! （`editor::material_ops`），这样本模块可被单测直接驱动，也不把文件系统拖进资产层。
//!
//! 每次提交都是「克隆 → 修改 → 用与导入/运行时同一台解析器回读 → 通过才替换」。
//! 用序列化回读而不是手写一套范围检查，是为了让编辑器接受的取值与 `.material` 文件、
//! 导入器、渲染器接受的取值**永远同源**；回读失败时会话保持原样，也不产生撤销步。

use crate::mat_asset::{MaterialAsset, MaterialAssetError};

/// 撤销栈上限（与 `.anim` 编辑会话一致；只保留最近的操作）。
pub const UNDO_LIMIT: usize = 128;

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum MaterialEditError {
    #[error("{0}")]
    Invalid(#[from] MaterialAssetError),
    /// 内存里的贴图是 glTF 内嵌像素（没有项目相对路径），写回 `.material` 会丢图。
    #[error("材质含内嵌贴图，无法保存为 .material（请先创建带贴图路径的覆盖）")]
    EmbeddedTexture,
}

/// `.material` 的可撤销编辑会话。
#[derive(Clone, Debug)]
pub struct MaterialEditSession {
    material: MaterialAsset,
    saved: MaterialAsset,
    undo: Vec<MaterialAsset>,
    redo: Vec<MaterialAsset>,
    /// 上一次提交是否处于同一段连续手势（拖动数值）里；用于把拖拽合并成一条撤销。
    coalescing: bool,
}

impl MaterialEditSession {
    #[must_use]
    pub fn new(material: MaterialAsset) -> Self {
        Self {
            saved: material.clone(),
            material,
            undo: Vec::new(),
            redo: Vec::new(),
            coalescing: false,
        }
    }

    #[must_use]
    pub fn material(&self) -> &MaterialAsset {
        &self.material
    }

    #[must_use]
    pub fn saved(&self) -> &MaterialAsset {
        &self.saved
    }

    #[must_use]
    pub fn is_dirty(&self) -> bool {
        self.material != self.saved
    }

    #[must_use]
    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    #[must_use]
    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    #[must_use]
    pub fn undo_depth(&self) -> usize {
        self.undo.len()
    }

    /// 把当前内容记为「已保存」。撤销历史保留：越过保存点后 `is_dirty()` 会重新变真。
    pub fn mark_saved(&mut self) {
        self.saved = self.material.clone();
        self.coalescing = false;
    }

    /// 在当前内容上做一次修改。校验失败时返回错误且不修改会话。
    pub fn edit(
        &mut self,
        mutate: impl FnOnce(&mut MaterialAsset),
    ) -> Result<(), MaterialEditError> {
        let mut next = self.material.clone();
        mutate(&mut next);
        self.commit(next, false)
    }

    /// 拖动数值控件时的连续修改：与上一段连续手势合并成**一条**撤销。
    ///
    /// 面板在 `drag_started` 时调 [`Self::break_coalescing`] 划界；键盘输入/选择框用
    /// [`Self::edit`]（每次都独立入栈）。
    pub fn edit_coalesced(
        &mut self,
        mutate: impl FnOnce(&mut MaterialAsset),
    ) -> Result<(), MaterialEditError> {
        let mut next = self.material.clone();
        mutate(&mut next);
        self.commit(next, true)
    }

    /// 结束当前连续手势：下一次编辑一定新开一条撤销。
    pub fn break_coalescing(&mut self) {
        self.coalescing = false;
    }

    /// 撤销一步；返回是否真的撤销了。
    pub fn undo(&mut self) -> bool {
        let Some(previous) = self.undo.pop() else {
            return false;
        };
        self.coalescing = false;
        self.redo
            .push(std::mem::replace(&mut self.material, previous));
        true
    }

    /// 重做一步；返回是否真的重做了。
    pub fn redo(&mut self) -> bool {
        let Some(next) = self.redo.pop() else {
            return false;
        };
        self.coalescing = false;
        self.undo.push(std::mem::replace(&mut self.material, next));
        true
    }

    /// 还原到上次保存的内容（一步可撤销）；未修改时返回 `false`。
    pub fn revert_to_saved(&mut self) -> Result<bool, MaterialEditError> {
        if !self.is_dirty() {
            return Ok(false);
        }
        self.coalescing = false;
        let saved = self.saved.clone();
        self.commit(saved, false)?;
        Ok(true)
    }

    fn commit(&mut self, next: MaterialAsset, coalesce: bool) -> Result<(), MaterialEditError> {
        if next == self.material {
            return Ok(());
        }
        let text = next.to_text();
        let reparsed = MaterialAsset::from_text("<edit>", &text)?;
        if reparsed != next {
            // `to_text` 只写贴图路径；内嵌图像会在往返中丢失。
            return Err(MaterialEditError::EmbeddedTexture);
        }
        if coalesce && self.coalescing && !self.undo.is_empty() {
            // 同一段拖动：只更新当前值，不新增撤销步（拖拽结束后的撤销仍回到手势起点）。
            self.material = next;
            return Ok(());
        }
        self.undo.push(std::mem::replace(&mut self.material, next));
        if self.undo.len() > UNDO_LIMIT {
            self.undo.remove(0);
        }
        self.redo.clear();
        self.coalescing = coalesce;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mat_asset::{MaterialAlphaMode, MaterialTexture};

    fn textured() -> MaterialAsset {
        MaterialAsset {
            name: Some("rusted".to_owned()),
            base_color: [0.8, 0.7, 0.6, 1.0],
            metallic: 1.0,
            roughness: 0.35,
            base_color_texture: Some(MaterialTexture {
                path: Some("assets/textures/rust.png".to_owned()),
                image: None,
            }),
            ..MaterialAsset::default()
        }
    }

    #[test]
    fn edits_are_undoable_and_redoable_byte_exactly() {
        let mut session = MaterialEditSession::new(textured());
        let before = session.material().to_text();
        session
            .edit(|material| material.roughness = 0.9)
            .expect("roughness in range");
        assert!(session.is_dirty());
        assert_eq!(session.undo_depth(), 1);
        assert_eq!(session.material().roughness, 0.9);

        assert!(session.undo());
        assert_eq!(session.material().to_text(), before);
        assert!(!session.is_dirty());
        assert!(session.redo());
        assert_eq!(session.material().roughness, 0.9);
        assert!(session.is_dirty());
    }

    #[test]
    fn rejected_edits_leave_no_trace() {
        let mut session = MaterialEditSession::new(textured());
        let before = session.material().clone();

        let error = session
            .edit(|material| material.metallic = 1.5)
            .expect_err("metallic > 1 must be rejected");
        assert!(error.to_string().contains("metallic"), "{error}");
        assert_eq!(session.material(), &before, "被拒绝的修改不得部分生效");
        assert!(!session.can_undo(), "被拒绝的修改不得进入撤销栈");
        assert!(!session.is_dirty());

        session
            .edit(|material| material.base_color[0] = f32::NAN)
            .expect_err("NaN must be rejected");
        session
            .edit(|material| material.alpha_mode = MaterialAlphaMode::Mask)
            .expect("valid alpha mode");
    }

    #[test]
    fn texture_paths_must_stay_inside_the_project() {
        let mut session = MaterialEditSession::new(textured());
        for bad in ["/abs/tex.png", "../escape.png", "assets/tex.bmp"] {
            let error = session
                .edit(|material| {
                    material.normal_texture = Some(MaterialTexture {
                        path: Some(bad.to_owned()),
                        image: None,
                    });
                })
                .expect_err("非法贴图路径必须被拒绝");
            assert!(
                error.to_string().contains("invalid texture path"),
                "{error}"
            );
        }
        assert!(!session.can_undo());
    }

    #[test]
    fn a_material_with_embedded_pixels_cannot_be_saved_as_text() {
        let image = crate::gltf_asset::GltfTextureImage {
            width: 1,
            height: 1,
            rgba: vec![255, 0, 0, 255],
        };
        let mut material = textured();
        material.base_color_texture = Some(MaterialTexture {
            path: None,
            image: Some(image),
        });
        let mut session = MaterialEditSession::new(material);
        let error = session
            .edit(|material| material.roughness = 0.2)
            .expect_err("内嵌贴图无法往返");
        assert_eq!(error, MaterialEditError::EmbeddedTexture);
    }

    #[test]
    fn empty_names_and_negative_strengths_are_rejected() {
        let mut session = MaterialEditSession::new(textured());
        session
            .edit(|material| material.name = Some(String::new()))
            .expect_err("空名称必须被拒绝");
        session
            .edit(|material| material.emissive_strength = -1.0)
            .expect_err("负发光强度必须被拒绝");
        session
            .edit(|material| material.ibl_intensity = -0.5)
            .expect_err("负 IBL 强度必须被拒绝");
        assert!(!session.can_undo());
    }

    #[test]
    fn marking_saved_keeps_history_so_undo_can_dirty_again() {
        let mut session = MaterialEditSession::new(textured());
        session
            .edit(|material| material.roughness = 0.5)
            .expect("edit");
        session.mark_saved();
        assert!(!session.is_dirty());
        assert!(session.undo());
        assert!(session.is_dirty(), "撤销越过保存点后必须重新变脏");
        assert!(session.redo());
        assert!(!session.is_dirty());

        session
            .edit(|material| material.metallic = 0.1)
            .expect("edit");
        assert!(session.revert_to_saved().expect("revert"));
        assert!(!session.is_dirty());
        assert_eq!(session.material().metallic, 1.0);
        assert!(session.undo(), "还原本身也是一步可撤销操作");
    }

    #[test]
    fn identical_edits_do_not_push_history() {
        let mut session = MaterialEditSession::new(textured());
        session
            .edit(|material| material.roughness = 0.35)
            .expect("no-op");
        assert!(!session.can_undo());
        assert!(!session.is_dirty());
    }

    #[test]
    fn a_drag_gesture_coalesces_into_one_undo_step() {
        let mut session = MaterialEditSession::new(textured());
        session
            .edit_coalesced(|material| material.roughness = 0.4)
            .expect("drag start");
        session
            .edit_coalesced(|material| material.roughness = 0.5)
            .expect("drag move");
        session
            .edit_coalesced(|material| material.roughness = 0.6)
            .expect("drag move");
        assert_eq!(session.undo_depth(), 1, "同一段拖动只留一条撤销");
        assert!(session.undo());
        assert_eq!(session.material().roughness, 0.35, "撤销回到手势起点");

        session
            .edit_coalesced(|material| material.roughness = 0.2)
            .expect("second drag");
        session.break_coalescing();
        session
            .edit_coalesced(|material| material.roughness = 0.1)
            .expect("third drag");
        assert_eq!(session.undo_depth(), 2, "划界后的新手势独立入栈");
    }
}
