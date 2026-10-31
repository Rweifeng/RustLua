//! Prefab assets and repeatable scene instantiation with explicit overrides.

use crate::scene::{Scene, SceneEntity, SceneEntityId, SceneError};
use rustlua_assets::resource_db::AssetId;
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    str::FromStr,
};

pub const CURRENT_PREFAB_VERSION: u32 = 1;

/// 实体元数据键：该实体是哪个 Prefab 源的实例（编辑器与运行时共用这个约定）。
pub const PREFAB_SOURCE_META_KEY: &str = "prefab.source";

/// 实体元数据键：嵌套实例的节点在**外层**预制体文件里对应哪个实体。
///
/// 只在"这个节点属于另一张预制体"时存在。为什么需要它：外层文件里每个实体都有自己的编号，
/// 而嵌套节点自己的 `prefab.source` / `prefab.source_id` 指的是**内层**文件，
/// 单靠那两个键回答不了"外层把我记成了几号"。外层重建实例时正是按这个编号把场景节点
/// 对回外层文件的实体（见 `editor::actions::prefab_rebuild`）。
pub const PREFAB_OUTER_ID_META_KEY: &str = "prefab.outer_id";

/// 文件级元数据键：这份预制体是**哪份预制体的变体**（工程内相对路径）。
///
/// 变体文件本身仍是完整的快照——运行时、旧编辑器、资源扫描读到的都是这份快照，
/// 不解析谱系。谱系只给编辑器用：让它知道"从谁出发、改过哪些字段"，从而能同步基础改动
/// 而不把基础的新值误判成自己的覆盖。
pub const PREFAB_VARIANT_BASE_META_KEY: &str = "prefab.variant_base";

/// 文件级元数据键：上次同步/保存时基础文件的签名。缺了它就不敢同步——
/// 无法区分"基础没动过"和"基础变了但我没看过"。
pub const PREFAB_VARIANT_BASE_STAMP_META_KEY: &str = "prefab.variant_base_stamp";

/// 文件级元数据键前缀：变体相对基础**显式改过**的字段。
///
/// 完整键为 `prefab.override.<实体编号>.<字段>`，字段语法与实例覆盖完全一致
/// （`name` / `component:<C>` / `property:<C>.<field>` / `metadata:<k>`），
/// 因此同一套 `apply_overrides` 就能把它叠到新快照上。
pub const PREFAB_OVERRIDE_META_PREFIX: &str = "prefab.override.";

/// 文件级元数据键前缀：嵌套内层预制体上次同步时的签名。键为
/// `prefab.nested_stamp.<内层工程内路径>`，值是签名文本（编辑器定义格式，这里不解释）。
pub const PREFAB_NESTED_STAMP_META_PREFIX: &str = "prefab.nested_stamp.";

/// 文件级元数据键前缀：外层相对**内层源**显式改过的字段。
///
/// 完整键为 `prefab.nested_override.<外层实体编号>.<字段>`——编号是**外层文件**里的实体
/// 编号（不是内层编号），值语法与实例覆盖一致（`name` / `component:<C>` /
/// `property:<C>.<field>` / `metadata:<k>`）。它回答"内层更新时哪些字段不能被覆盖"：
/// 没有记录的就是内层默认值，应当跟着内层走；有记录的才是外层的决定。
///
/// 与 [`PREFAB_OVERRIDE_META_PREFIX`] 的分工：那条描述"变体 vs 基础"，这条描述
/// "外层快照 vs 内层源"，因此可以同时出现在一份文件里（变体也可以嵌内层）。
pub const PREFAB_NESTED_OVERRIDE_META_PREFIX: &str = "prefab.nested_override.";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Prefab {
    pub version: u32,
    pub asset_id: AssetId,
    pub name: String,
    pub root: SceneEntityId,
    pub entities: BTreeMap<SceneEntityId, SceneEntity>,
    pub metadata: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PrefabInstance {
    pub prefab_id: AssetId,
    pub instance_root: SceneEntityId,
    pub source_to_instance: BTreeMap<SceneEntityId, SceneEntityId>,
    pub overrides: BTreeMap<(SceneEntityId, String), String>,
}

impl Prefab {
    /// Creates a prefab from one scene entity and all descendants.
    pub fn from_scene(
        scene: &Scene,
        root: SceneEntityId,
        asset_id: AssetId,
        name: impl Into<String>,
    ) -> Result<Self, PrefabError> {
        let root_entity = scene
            .entities
            .get(&root)
            .ok_or(PrefabError::MissingEntity(root))?;
        let mut ids = Vec::new();
        collect_ids(scene, root, &mut ids)?;
        let mut entities = BTreeMap::new();
        for id in ids {
            let mut entity = scene
                .entities
                .get(&id)
                .expect("collected entity exists")
                .clone();
            if id == root {
                entity.parent = None;
            }

            entities.insert(id, entity);
        }
        // Rebuild children after collection so ordering is independent of insertion order.
        let entity_ids: std::collections::BTreeSet<_> = entities.keys().copied().collect();
        for entity in entities.values_mut() {
            entity.children.retain(|id| entity_ids.contains(id));
        }
        // **文件根永远属于这张预制体自己**：嵌套标记只能落在后代上。
        // 从"一个内层实例的根"创建预制体时，那份标记若留在文件根上，实例化后这个节点
        // 同时是两张预制体的根，`prefab_root_ids` 按来源找不到它，源改动就同步不到场景。
        if let Some(root_entity) = entities.get_mut(&root) {
            for key in [
                PREFAB_SOURCE_META_KEY,
                "prefab.source_id",
                "prefab.root",
                PREFAB_OUTER_ID_META_KEY,
            ] {
                root_entity.metadata.remove(key);
            }
        }
        let _ = root_entity;
        Ok(Self {
            version: CURRENT_PREFAB_VERSION,
            asset_id,
            name: name.into(),
            root,
            entities,
            metadata: BTreeMap::new(),
        })
    }

    /// Instantiates a fresh copy and applies all saved instance overrides.
    pub fn instantiate(
        &self,
        scene: &mut Scene,
        parent: Option<SceneEntityId>,
        overrides: &BTreeMap<(SceneEntityId, String), String>,
    ) -> Result<PrefabInstance, PrefabError> {
        self.validate()?;
        if let Some(parent_id) = parent
            && !scene.entities.contains_key(&parent_id)
        {
            return Err(PrefabError::Scene(SceneError::MissingParent(parent_id)));
        }
        let mut map = BTreeMap::new();
        let mut next = scene
            .entities
            .keys()
            .map(|id| id.0)
            .max()
            .unwrap_or(0)
            .saturating_add(1);
        for source_id in self.entities.keys() {
            map.insert(*source_id, SceneEntityId(next));
            next = next.checked_add(1).ok_or(PrefabError::IdExhausted)?;
        }
        for (source_id, source_entity) in &self.entities {
            let instance_id = map[source_id];
            let mapped_parent = if *source_id == self.root {
                parent
            } else {
                source_entity.parent.map(|id| map[&id])
            };
            let mut entity = source_entity.clone();
            entity.id = instance_id;
            entity.parent = mapped_parent;
            entity.children = source_entity.children.iter().map(|id| map[id]).collect();
            scene.entities.insert(instance_id, entity);
        }
        if let Some(parent_id) = parent {
            scene
                .entities
                .get_mut(&parent_id)
                .expect("validated parent")
                .children
                .push(map[&self.root]);
        }
        let instance = PrefabInstance {
            prefab_id: self.asset_id,
            instance_root: map[&self.root],
            source_to_instance: map,
            overrides: overrides.clone(),
        };
        instance.apply_overrides(scene)?;
        Ok(instance)
    }

    /// 这份 Prefab **内部**引用的其它 Prefab 源（`prefab.source` 元数据的去重值）。
    ///
    /// 非空就表示它是一份**嵌套 Prefab**：根节点自己带 `prefab.source`（本 Prefab 的根是另一个
    /// Prefab 的实例），或者树里某个后代是别的 Prefab 的实例。
    /// 嵌套节点在本文件里存的是**内层实例当时的取值快照**加它自己的内层标记
    /// （`prefab.source` / `prefab.source_id` / `prefab.root`），所以实例化与重建都不需要
    /// 再展开内层文件；这条信息因此是"该给什么徽标、该拒什么环"的事实源
    /// （见 `rustlua_editor_core::prefab_graph` 的扫描器）。
    #[must_use]
    pub fn nested_sources(&self) -> Vec<String> {
        let mut sources: Vec<String> = self
            .entities
            .values()
            .filter_map(|entity| entity.metadata.get(PREFAB_SOURCE_META_KEY))
            .filter(|source| !source.trim().is_empty())
            .cloned()
            .collect();
        sources.sort();
        sources.dedup();
        sources
    }

    /// 变体谱系：基础预制体的工程内相对路径。
    #[must_use]
    pub fn variant_base(&self) -> Option<&str> {
        self.metadata
            .get(PREFAB_VARIANT_BASE_META_KEY)
            .map(String::as_str)
            .filter(|value| !value.trim().is_empty())
    }

    /// 设置基础谱系。换基础等于换基线，旧 stamp 立即作废。
    pub fn set_variant_base(&mut self, base: Option<&str>) -> Result<(), PrefabError> {
        self.metadata.remove(PREFAB_VARIANT_BASE_META_KEY);
        self.metadata.remove(PREFAB_VARIANT_BASE_STAMP_META_KEY);
        if let Some(base) = base {
            validate_prefab_ref(PREFAB_VARIANT_BASE_META_KEY, base)?;
            self.metadata
                .insert(PREFAB_VARIANT_BASE_META_KEY.to_owned(), base.to_owned());
        }
        Ok(())
    }

    /// 上次同步/保存时基础文件的签名。
    #[must_use]
    pub fn variant_base_stamp(&self) -> Option<&str> {
        self.metadata
            .get(PREFAB_VARIANT_BASE_STAMP_META_KEY)
            .map(String::as_str)
            .filter(|value| !value.trim().is_empty())
    }

    pub fn set_variant_base_stamp(&mut self, stamp: Option<&str>) -> Result<(), PrefabError> {
        self.metadata.remove(PREFAB_VARIANT_BASE_STAMP_META_KEY);
        if let Some(stamp) = stamp {
            if stamp.trim().is_empty() {
                return Err(PrefabError::InvalidVariant("基础签名不能为空".to_owned()));
            }
            self.metadata.insert(
                PREFAB_VARIANT_BASE_STAMP_META_KEY.to_owned(),
                stamp.to_owned(),
            );
        }
        Ok(())
    }

    /// 变体相对基础显式改过的字段（键与实例覆盖同构：实体编号 + 字段语法）。
    ///
    /// 键写坏时报错而不是跳过：谱系记录是同步的依据，静默丢弃会让下次同步把用户改动抹掉。
    pub fn overrides(&self) -> Result<BTreeMap<(SceneEntityId, String), String>, PrefabError> {
        let mut out = BTreeMap::new();
        for (key, value) in &self.metadata {
            let Some(rest) = key.strip_prefix(PREFAB_OVERRIDE_META_PREFIX) else {
                continue;
            };
            let (entity_id, field) = override_key_parts(key, rest)?;
            out.insert((entity_id, field.to_owned()), value.clone());
        }
        Ok(out)
    }

    /// 整体替换覆盖记录。逐条按 `apply_overrides` 的口径校验，避免写出实例化时会失败的文件。
    pub fn set_overrides(
        &mut self,
        overrides: &BTreeMap<(SceneEntityId, String), String>,
    ) -> Result<(), PrefabError> {
        for ((entity_id, field), value) in overrides {
            let entity = self
                .entities
                .get(entity_id)
                .ok_or(PrefabError::MissingEntity(*entity_id))?;
            validate_override_field(*entity_id, entity, field, value)?;
        }
        self.metadata
            .retain(|key, _| !key.starts_with(PREFAB_OVERRIDE_META_PREFIX));
        for ((entity_id, field), value) in overrides {
            self.metadata.insert(
                format!("{PREFAB_OVERRIDE_META_PREFIX}{}.{field}", entity_id.0),
                value.clone(),
            );
        }
        Ok(())
    }

    /// 外层相对内层源显式改过的字段（键是外层文件实体编号 + 实例覆盖字段语法）。
    ///
    /// 键写坏时报错：这些记录决定内层更新时哪些字段必须保留，静默丢弃会让下次同步
    /// 把作者的外层改动抹掉。
    pub fn nested_overrides(
        &self,
    ) -> Result<BTreeMap<(SceneEntityId, String), String>, PrefabError> {
        let mut out = BTreeMap::new();
        for (key, value) in &self.metadata {
            let Some(rest) = key.strip_prefix(PREFAB_NESTED_OVERRIDE_META_PREFIX) else {
                continue;
            };
            let (entity_id, field) = override_key_parts(key, rest)?;
            out.insert((entity_id, field.to_owned()), value.clone());
        }
        Ok(out)
    }

    /// 整体替换嵌套覆盖记录。逐条校验：目标必须是嵌套节点，字段必须能套到它的快照上。
    pub fn set_nested_overrides(
        &mut self,
        overrides: &BTreeMap<(SceneEntityId, String), String>,
    ) -> Result<(), PrefabError> {
        for ((entity_id, field), value) in overrides {
            let entity = self
                .entities
                .get(entity_id)
                .ok_or(PrefabError::MissingEntity(*entity_id))?;
            if !entity.metadata.contains_key(PREFAB_SOURCE_META_KEY) {
                return Err(PrefabError::InvalidOverride(format!(
                    "实体 {} 不是嵌套预制体节点，不能记录嵌套覆盖（{field}）",
                    entity_id.0
                )));
            }
            validate_override_field(*entity_id, entity, field, value)?;
        }
        self.metadata
            .retain(|key, _| !key.starts_with(PREFAB_NESTED_OVERRIDE_META_PREFIX));
        for ((entity_id, field), value) in overrides {
            self.metadata.insert(
                format!(
                    "{PREFAB_NESTED_OVERRIDE_META_PREFIX}{}.{field}",
                    entity_id.0
                ),
                value.clone(),
            );
        }
        Ok(())
    }

    /// 某个内层来源上次同步时的签名。
    #[must_use]
    pub fn nested_stamp(&self, source: &str) -> Option<&str> {
        self.metadata
            .get(&format!("{PREFAB_NESTED_STAMP_META_PREFIX}{source}"))
            .map(String::as_str)
    }

    pub fn set_nested_stamp(
        &mut self,
        source: &str,
        stamp: Option<&str>,
    ) -> Result<(), PrefabError> {
        validate_prefab_ref(PREFAB_NESTED_STAMP_META_PREFIX, source)?;
        let key = format!("{PREFAB_NESTED_STAMP_META_PREFIX}{source}");
        self.metadata.remove(&key);
        if let Some(stamp) = stamp {
            if stamp.trim().is_empty() {
                return Err(PrefabError::InvalidVariant(format!(
                    "内层签名不能为空：{source}"
                )));
            }
            self.metadata.insert(key, stamp.to_owned());
        }
        Ok(())
    }

    /// 全部内层签名（按来源排序，文本输出因此稳定）。
    #[must_use]
    pub fn nested_stamps(&self) -> BTreeMap<String, String> {
        self.metadata
            .iter()
            .filter_map(|(key, value)| {
                key.strip_prefix(PREFAB_NESTED_STAMP_META_PREFIX)
                    .filter(|source| !source.trim().is_empty())
                    .map(|source| (source.to_owned(), value.clone()))
            })
            .collect()
    }

    pub fn save(&self, path: impl AsRef<Path>) -> Result<(), PrefabError> {
        self.validate()?;
        let path = path.as_ref();
        let temp = path.with_extension("tmp");
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&temp, self.to_text())?;
        fs::rename(temp, path)?;
        Ok(())
    }
    pub fn load(path: impl AsRef<Path>) -> Result<Self, PrefabError> {
        let path = path.as_ref();
        let text = fs::read_to_string(path).map_err(|e| PrefabError::Io {
            path: path.display().to_string(),
            reason: e.to_string(),
        })?;
        Self::from_text(&text).map_err(|e| e.with_path(path))
    }
    pub fn to_text(&self) -> String {
        let mut out = format!(
            "prefab_version={}\nasset_id={}\nname={}\nroot={}\n",
            self.version,
            self.asset_id,
            escape(&self.name),
            self.root.0
        );
        for (k, v) in &self.metadata {
            out.push_str(&format!("meta\t{}\t{}\n", escape(k), escape(v)));
        }
        for e in self.entities.values() {
            // 根实体**不写空的 parent 段**：与 `.scene` 同一条口径（那边是 3 段或 4 段），
            // 也躲开尾随制表符——`git diff --check` 与 CI 的 'Diff check' 都判它脏。
            match e.parent {
                Some(parent) => out.push_str(&format!(
                    "entity\t{}\t{}\t{}\n",
                    e.id.0,
                    escape(&e.name),
                    parent.0
                )),
                None => out.push_str(&format!("entity\t{}\t{}\n", e.id.0, escape(&e.name))),
            }
            for (k, v) in &e.components {
                out.push_str(&format!(
                    "component\t{}\t{}\t{}\n",
                    e.id.0,
                    escape(k),
                    escape(v)
                ));
            }
            for (k, v) in &e.resources {
                out.push_str(&format!("resource\t{}\t{}\t{}\n", e.id.0, escape(k), v));
            }
            for (k, v) in &e.metadata {
                out.push_str(&format!(
                    "entity_meta\t{}\t{}\t{}\n",
                    e.id.0,
                    escape(k),
                    escape(v)
                ));
            }
        }
        out
    }
    pub fn from_text(text: &str) -> Result<Self, PrefabError> {
        let mut version = None;
        let mut asset = None;
        let mut name = None;
        let mut root = None;
        let mut metadata = BTreeMap::new();
        let mut entities = BTreeMap::new();
        for (n, line) in text.lines().enumerate() {
            let line_no = n + 1;
            if line.trim().is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some(v) = line.strip_prefix("prefab_version=") {
                version = Some(v.parse().map_err(|_| PrefabError::Parse {
                    line: line_no,
                    reason: "invalid prefab_version".into(),
                    path: None,
                })?);
                continue;
            }
            if let Some(v) = line.strip_prefix("asset_id=") {
                asset = Some(AssetId::from_str(v).map_err(|_| PrefabError::Parse {
                    line: line_no,
                    reason: "invalid asset_id".into(),
                    path: None,
                })?);
                continue;
            }
            if let Some(v) = line.strip_prefix("name=") {
                name = Some(unescape(v, line_no)?);
                continue;
            }
            if let Some(v) = line.strip_prefix("root=") {
                root = Some(SceneEntityId(number(v, line_no)?));
                continue;
            }
            let f: Vec<_> = line.split('\t').collect();
            match f.first().copied() {
                Some("meta") => {
                    need(&f, 3, line_no)?;
                    metadata.insert(unescape(f[1], line_no)?, unescape(f[2], line_no)?);
                }
                Some("entity") => {
                    // 与 `scene.rs` 同一条口径：根实体 3 段、子实体 4 段（parent 可缺省，
                    // 缺省时不写那个空字段，见 `to_text`）。
                    if f.len() != 3 && f.len() != 4 {
                        return Err(PrefabError::Parse {
                            line: line_no,
                            reason: format!("entity expected 3 or 4 fields, got {}", f.len()),
                            path: None,
                        });
                    }
                    let id = SceneEntityId(number(f[1], line_no)?);
                    entities.insert(
                        id,
                        SceneEntity {
                            id,
                            name: unescape(f[2], line_no)?,
                            parent: match f.get(3).copied() {
                                None | Some("") => None,
                                Some(raw) => Some(SceneEntityId(number(raw, line_no)?)),
                            },
                            children: Vec::new(),
                            components: BTreeMap::new(),
                            resources: BTreeMap::new(),
                            metadata: BTreeMap::new(),
                        },
                    );
                }
                Some("component") | Some("resource") | Some("entity_meta") => {
                    need(&f, 4, line_no)?;
                    let id = SceneEntityId(number(f[1], line_no)?);
                    let e = entities.get_mut(&id).ok_or(PrefabError::Parse {
                        line: line_no,
                        reason: "record references unknown entity".into(),
                        path: None,
                    })?;
                    let key = unescape(f[2], line_no)?;
                    match f[0] {
                        "component" => {
                            e.components.insert(key, unescape(f[3], line_no)?);
                        }
                        "resource" => {
                            e.resources.insert(
                                key,
                                AssetId::from_str(f[3]).map_err(|_| PrefabError::Parse {
                                    line: line_no,
                                    reason: "invalid resource UUID".into(),
                                    path: None,
                                })?,
                            );
                        }
                        _ => {
                            e.metadata.insert(key, unescape(f[3], line_no)?);
                        }
                    }
                }
                _ => {
                    return Err(PrefabError::Parse {
                        line: line_no,
                        reason: "unknown record".into(),
                        path: None,
                    });
                }
            }
        }
        let prefab = Self {
            version: version.ok_or(PrefabError::MissingField("prefab_version".into()))?,
            asset_id: asset.ok_or(PrefabError::MissingField("asset_id".into()))?,
            name: name.ok_or(PrefabError::MissingField("name".into()))?,
            root: root.ok_or(PrefabError::MissingField("root".into()))?,
            entities,
            metadata,
        };
        let mut prefab = prefab;
        for e in prefab.entities.values_mut() {
            e.children.clear();
        }
        let ids: Vec<_> = prefab.entities.keys().copied().collect();
        for id in ids {
            if let Some(parent) = prefab.entities[&id].parent {
                prefab
                    .entities
                    .get_mut(&parent)
                    .ok_or(PrefabError::MissingEntity(parent))?
                    .children
                    .push(id);
            }
        }
        prefab.validate()?;
        Ok(prefab)
    }
    pub fn validate(&self) -> Result<(), PrefabError> {
        if self.version != CURRENT_PREFAB_VERSION {
            return Err(PrefabError::UnsupportedVersion(self.version));
        }
        if !self.entities.contains_key(&self.root) {
            return Err(PrefabError::MissingEntity(self.root));
        }
        for (id, e) in &self.entities {
            if let Some(parent) = e.parent {
                let p = self
                    .entities
                    .get(&parent)
                    .ok_or(PrefabError::MissingEntity(parent))?;
                if !p.children.contains(id) {
                    return Err(PrefabError::TreeMismatch(*id));
                }
            }
        }
        self.validate_lineage()?;
        Ok(())
    }

    /// 谱系/覆盖记录的自洽性：基础与签名成对出现、覆盖指向存在的实体且字段可应用、
    /// 内层签名只落在合法的工程内路径上。
    ///
    /// 放在 `validate` 里（因此 `load`/`save`/`instantiate` 都会经过）：这些记录直接决定
    /// 编辑器下次同步时的行为，坏一条就该在入口处被挡住，而不是等同步时才炸。
    fn validate_lineage(&self) -> Result<(), PrefabError> {
        match self.variant_base() {
            Some(base) => {
                validate_prefab_ref(PREFAB_VARIANT_BASE_META_KEY, base)?;
                if self.variant_base_stamp().is_none() {
                    return Err(PrefabError::InvalidVariant(format!(
                        "变体记录缺少基础签名（{PREFAB_VARIANT_BASE_STAMP_META_KEY}）：{base}"
                    )));
                }
            }
            None => {
                if self
                    .metadata
                    .contains_key(PREFAB_VARIANT_BASE_STAMP_META_KEY)
                {
                    return Err(PrefabError::InvalidVariant(
                        "只有基础签名、没有基础路径".to_owned(),
                    ));
                }
            }
        }
        for (key, value) in &self.metadata {
            if let Some(rest) = key.strip_prefix(PREFAB_OVERRIDE_META_PREFIX) {
                let (entity_id, field) = override_key_parts(key, rest)?;
                let entity = self.entities.get(&entity_id).ok_or_else(|| {
                    PrefabError::InvalidOverride(format!("{key}: 引用了不存在的实体"))
                })?;
                validate_override_field(entity_id, entity, field, value)?;
            } else if let Some(source) = key.strip_prefix(PREFAB_NESTED_STAMP_META_PREFIX) {
                validate_prefab_ref(PREFAB_NESTED_STAMP_META_PREFIX, source)?;
                if value.trim().is_empty() {
                    return Err(PrefabError::InvalidVariant(format!(
                        "内层签名不能为空：{source}"
                    )));
                }
            } else if let Some(rest) = key.strip_prefix(PREFAB_NESTED_OVERRIDE_META_PREFIX) {
                let (entity_id, field) = override_key_parts(key, rest)?;
                let entity = self.entities.get(&entity_id).ok_or_else(|| {
                    PrefabError::InvalidOverride(format!("{key}: 引用了不存在的实体"))
                })?;
                if !entity.metadata.contains_key(PREFAB_SOURCE_META_KEY) {
                    return Err(PrefabError::InvalidOverride(format!(
                        "{key}: 目标不是嵌套预制体节点"
                    )));
                }
                validate_override_field(entity_id, entity, field, value)?;
            }
        }
        Ok(())
    }
}

impl PrefabInstance {
    pub fn set_override(
        &mut self,
        source_entity: SceneEntityId,
        field: impl Into<String>,
        value: impl Into<String>,
    ) {
        self.overrides
            .insert((source_entity, field.into()), value.into());
    }
    pub fn apply_overrides(&self, scene: &mut Scene) -> Result<(), PrefabError> {
        for ((source, field), value) in &self.overrides {
            let id = *self
                .source_to_instance
                .get(source)
                .ok_or(PrefabError::MissingEntity(*source))?;
            let e = scene
                .entities
                .get_mut(&id)
                .ok_or(PrefabError::MissingEntity(id))?;
            if field == "name" {
                e.name = value.clone();
            } else if let Some(key) = field.strip_prefix("component:") {
                e.components.insert(key.to_owned(), value.clone());
            } else if let Some(path) = field.strip_prefix("property:") {
                // docs/55 阶段 3：逐字段 Override。只重写目标字段，其余字段保持逐字不变，
                // 因此"改了一个字段"不会把整个组件串换掉（也不会把别人的覆盖一起吞掉）。
                let path = rustlua_ecs::property_path::PropertyPath::parse(path)
                    .map_err(|error| PrefabError::InvalidOverride(format!("{field}: {error}")))?;
                let Some(current) = e.components.get(&path.component).cloned() else {
                    return Err(PrefabError::InvalidOverride(format!(
                        "{field}: 实例缺少组件 {}",
                        path.component
                    )));
                };
                let updated = rustlua_ecs::property_path::write_builtin(&current, &path, value)
                    .map_err(|error| PrefabError::InvalidOverride(format!("{field}: {error}")))?;
                e.components.insert(path.component, updated);
            } else if let Some(key) = field.strip_prefix("metadata:") {
                e.metadata.insert(key.to_owned(), value.clone());
            } else {
                return Err(PrefabError::InvalidOverride(field.clone()));
            }
        }
        Ok(())
    }
    /// Rebuild policy: callers create a new copy from the source and reapply these overrides.
    pub fn rebuild(
        &self,
        prefab: &Prefab,
        scene: &mut Scene,
        parent: Option<SceneEntityId>,
    ) -> Result<Self, PrefabError> {
        prefab.instantiate(scene, parent, &self.overrides)
    }
}

fn invalid_variant(reason: impl Into<String>) -> PrefabError {
    PrefabError::InvalidVariant(reason.into())
}

/// 谱系引用只能是工程内相对的 `.prefab` 路径：绝对路径与 `..` 会把同步写到工程外面。
fn validate_prefab_ref(label: &str, value: &str) -> Result<(), PrefabError> {
    let path = Path::new(value);
    let bad = value.trim().is_empty()
        || value.contains('\\')
        || path.is_absolute()
        || path.extension().and_then(|ext| ext.to_str()) != Some("prefab")
        || !path
            .components()
            .all(|component| matches!(component, std::path::Component::Normal(_)));
    if bad {
        return Err(invalid_variant(format!(
            "{label} 必须是工程内相对的 .prefab 路径：{value}"
        )));
    }
    Ok(())
}

/// 拆 `prefab.override.<实体编号>.<字段>`：实体编号取到第一个点，剩下整段都是字段
/// （`property:Sprite.tint[1]` 自己带点，不能按点再切）。
fn override_key_parts<'a>(
    key: &str,
    rest: &'a str,
) -> Result<(SceneEntityId, &'a str), PrefabError> {
    let (id, field) = rest
        .split_once('.')
        .ok_or_else(|| PrefabError::InvalidOverride(format!("{key}: 缺少实体编号或字段")))?;
    if id.is_empty() || field.trim().is_empty() {
        return Err(PrefabError::InvalidOverride(format!(
            "{key}: 缺少实体编号或字段"
        )));
    }
    let id = id
        .parse::<u64>()
        .map_err(|_| PrefabError::InvalidOverride(format!("{key}: 实体编号不是整数")))?;
    Ok((SceneEntityId(id), field))
}

/// 字段语法与 `PrefabInstance::apply_overrides` 逐条对齐：校验通过的文件一定能实例化。
///
/// `property:` 还会对着**快照里的组件串**试写一次值，把"加载时能过、实例化时炸"的坏数据
/// 挡在入口；`metadata:` 拒绝 `prefab.` 前缀——那些键是实例身份证，覆盖它们会把节点
/// 从嵌套树上摘下来。
fn validate_override_field(
    entity_id: SceneEntityId,
    entity: &SceneEntity,
    field: &str,
    value: &str,
) -> Result<(), PrefabError> {
    let fail = |reason: &str| {
        Err(PrefabError::InvalidOverride(format!(
            "实体 {}.{field}: {reason}",
            entity_id.0
        )))
    };
    if field == "name" {
        return Ok(());
    }
    if let Some(key) = field.strip_prefix("component:") {
        return if key.trim().is_empty() {
            fail("组件名为空")
        } else {
            Ok(())
        };
    }
    if let Some(path) = field.strip_prefix("property:") {
        let path = rustlua_ecs::property_path::PropertyPath::parse(path)
            .map_err(|error| PrefabError::InvalidOverride(format!("{field}: {error}")))?;
        let Some(payload) = entity.components.get(&path.component) else {
            return fail("快照里没有这个组件");
        };
        rustlua_ecs::property_path::write_builtin(payload, &path, value)
            .map_err(|error| PrefabError::InvalidOverride(format!("{field}: {error}")))?;
        return Ok(());
    }
    if let Some(key) = field.strip_prefix("metadata:") {
        if key.trim().is_empty() {
            return fail("元数据键为空");
        }
        if key.starts_with("prefab.") {
            return fail("不能覆盖预制体自身的元数据键");
        }
        return Ok(());
    }
    fail("不支持的覆盖字段")
}

fn collect_ids(
    scene: &Scene,
    id: SceneEntityId,
    out: &mut Vec<SceneEntityId>,
) -> Result<(), PrefabError> {
    let e = scene
        .entities
        .get(&id)
        .ok_or(PrefabError::MissingEntity(id))?;
    out.push(id);
    for child in &e.children {
        collect_ids(scene, *child, out)?;
    }
    Ok(())
}
fn need(f: &[&str], n: usize, line: usize) -> Result<(), PrefabError> {
    if f.len() != n {
        Err(PrefabError::Parse {
            line,
            reason: format!("expected {n} fields, got {}", f.len()),
            path: None,
        })
    } else {
        Ok(())
    }
}
fn number(v: &str, line: usize) -> Result<u64, PrefabError> {
    v.parse().map_err(|_| PrefabError::Parse {
        line,
        reason: "expected integer".into(),
        path: None,
    })
}
fn escape(v: &str) -> String {
    v.replace('\\', "\\\\")
        .replace('\t', "\\t")
        .replace('\n', "\\n")
}
fn unescape(v: &str, line: usize) -> Result<String, PrefabError> {
    let mut o = String::new();
    let mut c = v.chars();
    while let Some(x) = c.next() {
        if x != '\\' {
            o.push(x);
        } else {
            match c.next() {
                Some('n') => o.push('\n'),
                Some('t') => o.push('\t'),
                Some('\\') => o.push('\\'),
                _ => {
                    return Err(PrefabError::Parse {
                        line,
                        reason: "invalid escape".into(),
                        path: None,
                    });
                }
            }
        }
    }
    Ok(o)
}

#[derive(Debug, thiserror::Error)]
pub enum PrefabError {
    #[error("prefab I/O failed for {path}: {reason}")]
    Io { path: String, reason: String },
    #[error(
        "prefab parse error{} at line {line}: {reason}",
        .path.as_ref().map(|p| format!(" in {}", p.display())).unwrap_or_default()
    )]
    Parse {
        line: usize,
        reason: String,
        path: Option<PathBuf>,
    },
    #[error("missing prefab field: {0}")]
    MissingField(String),
    #[error("unsupported prefab version: {0}")]
    UnsupportedVersion(u32),
    #[error("missing prefab entity: {}", .0 .0)]
    MissingEntity(SceneEntityId),
    #[error("prefab tree mismatch: {}", .0 .0)]
    TreeMismatch(SceneEntityId),
    #[error(transparent)]
    Scene(#[from] SceneError),
    #[error("invalid prefab override: {0}")]
    InvalidOverride(String),
    #[error("invalid prefab variant lineage: {0}")]
    InvalidVariant(String),
    #[error("scene entity ID space exhausted")]
    IdExhausted,
}

impl PrefabError {
    fn with_path(mut self, path: &Path) -> Self {
        if let Self::Parse { path: slot, .. } = &mut self {
            *slot = Some(path.to_path_buf());
        }
        self
    }
}

impl From<std::io::Error> for PrefabError {
    fn from(e: std::io::Error) -> Self {
        Self::Io {
            path: "<prefab>".into(),
            reason: e.to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn scene() -> Scene {
        let mut s = Scene::new("source");
        s.add_entity(SceneEntityId(1), "Root", None).unwrap();
        s.add_entity(SceneEntityId(2), "Child", Some(SceneEntityId(1)))
            .unwrap();
        s.entities
            .get_mut(&SceneEntityId(2))
            .unwrap()
            .components
            .insert("Transform".into(), "x=1".into());
        s
    }
    #[test]
    fn creates_instantiates_and_round_trips() {
        let s = scene();
        let id = AssetId::from_seed("player.prefab");
        let p = Prefab::from_scene(&s, SceneEntityId(1), id, "Player").unwrap();
        let mut target = Scene::new("target");
        let mut instance = p.instantiate(&mut target, None, &BTreeMap::new()).unwrap();
        instance.set_override(SceneEntityId(2), "component:Transform", "x=9");
        instance.apply_overrides(&mut target).unwrap();
        assert_eq!(target.entities.len(), 2);
        assert_eq!(
            target.entities[&instance.source_to_instance[&SceneEntityId(2)]].components["Transform"],
            "x=9"
        );
        let text = p.to_text();
        // 根实体是 3 段（不写空的 parent 段）：尾随制表符会被 `git diff --check` 与 CI 的
        // 'Diff check' 判脏，而 `.scene` 那边本来就允许 3 段——两边必须是同一条口径。
        assert!(
            text.lines().any(|line| line == "entity\t1\tRoot"),
            "根实体行应为 3 段：{text}"
        );
        assert!(
            !text.lines().any(|line| line.ends_with('\t')),
            "预设文本有行尾制表符：{text}"
        );
        assert_eq!(Prefab::from_text(&text).unwrap(), p);
        // 段数检查没有被放宽成"随便几段"：2 段仍然拒。
        let two = text.replace("entity\t1\tRoot", "entity\t1");
        assert!(
            Prefab::from_text(&two).is_err(),
            "entity 只有 2 段时必须报错：{two}"
        );
    }
    #[test]
    fn property_overrides_rewrite_one_field_only() {
        // 逐字段 Override：只改 Sprite.tint[1]，其余字段（含 size 的 `x` 形式与其它分量）
        // 必须逐字保留，且写回同一个值时不产生任何变化。
        let mut s = Scene::new("source");
        s.add_entity(SceneEntityId(1), "Root", None).unwrap();
        s.entities.get_mut(&SceneEntityId(1)).unwrap().components.insert(
            "Sprite".into(),
            "texture=a.png;size=16x16;pivot=0.500,0.500;tint=1.000,1.000,1.000,1.000;sorting_layer=world;order=3"
                .into(),
        );
        let p = Prefab::from_scene(&s, SceneEntityId(1), AssetId::from_seed("p"), "P").unwrap();
        let mut target = Scene::new("t");
        let mut instance = p.instantiate(&mut target, None, &BTreeMap::new()).unwrap();

        instance.set_override(SceneEntityId(1), "property:Sprite.tint[1]", "0.25");
        instance.apply_overrides(&mut target).unwrap();
        let payload =
            target.entities[&instance.source_to_instance[&SceneEntityId(1)]].components["Sprite"]
                .clone();
        assert_eq!(
            payload,
            "texture=a.png;size=16x16;pivot=0.500,0.500;tint=1.000,0.25,1.000,1.000;sorting_layer=world;order=3"
        );

        // 再应用同一个覆盖：结果完全一致（幂等，无格式抖动）。
        instance.apply_overrides(&mut target).unwrap();
        let again =
            target.entities[&instance.source_to_instance[&SceneEntityId(1)]].components["Sprite"]
                .clone();
        assert_eq!(payload, again);

        // 非法字段值不会被写进场景。
        instance.set_override(SceneEntityId(1), "property:Sprite.order", "not-a-number");
        assert!(instance.apply_overrides(&mut target).is_err());
        // 未注册组件没有 schema：明确报错而不是静默忽略。
        instance.set_override(SceneEntityId(1), "property:Nope.x", "1");
        assert!(instance.apply_overrides(&mut target).is_err());
    }

    #[test]
    fn multiple_instances_get_unique_ids_and_rebuild_policy_preserves_override() {
        let s = scene();
        let p = Prefab::from_scene(&s, SceneEntityId(1), AssetId::from_seed("p"), "P").unwrap();
        let mut target = Scene::new("t");
        let a = p.instantiate(&mut target, None, &BTreeMap::new()).unwrap();
        let mut overrides = BTreeMap::new();
        overrides.insert((SceneEntityId(1), "name".into()), "Boss".into());
        let b = p.instantiate(&mut target, None, &overrides).unwrap();
        assert_ne!(a.instance_root, b.instance_root);
        assert_eq!(target.entities[&b.instance_root].name, "Boss");
        assert_eq!(b.overrides.len(), 1);
    }
    #[test]
    fn rejects_bad_version() {
        let text =
            "prefab_version=2\nasset_id=00000000-0000-4000-8000-000000000000\nname=x\nroot=1\n";
        assert!(matches!(
            Prefab::from_text(text),
            Err(PrefabError::UnsupportedVersion(2))
        ));
    }

    /// **嵌套 Prefab 的保真底线**：外层文件必须原样记住"哪些节点属于内层、属于哪张内层"。
    ///
    /// 内层标记（`prefab.source` / `prefab.source_id` / `prefab.root`）是嵌套实例唯一的身份证；
    /// 它们在 `from_scene` → `to_text` → `from_text` → `instantiate` 整条路上一个都不能丢，
    /// 否则实例化出来的内层子树会退化成普通节点，编辑器再也认不出它属于谁。
    #[test]
    fn nested_markers_survive_save_load_and_instantiate() {
        let mut inner = Scene::new("inner");
        inner.add_entity(SceneEntityId(1), "Bar", None).unwrap();
        let inner_prefab = Prefab::from_scene(
            &inner,
            SceneEntityId(1),
            AssetId::from_seed("bar.prefab"),
            "Bar",
        )
        .unwrap();

        // 场景：Root(1) 下挂一个普通节点(2) 与一个内层实例(3)。
        let mut scene = Scene::new("outer");
        scene.add_entity(SceneEntityId(1), "Root", None).unwrap();
        scene
            .add_entity(SceneEntityId(2), "Plain", Some(SceneEntityId(1)))
            .unwrap();
        let instance = inner_prefab
            .instantiate(&mut scene, Some(SceneEntityId(1)), &BTreeMap::new())
            .unwrap();
        for (source_id, entity_id) in &instance.source_to_instance {
            let entity = scene.entities.get_mut(entity_id).unwrap();
            entity.metadata.insert(
                PREFAB_SOURCE_META_KEY.to_owned(),
                "assets/bar.prefab".to_owned(),
            );
            entity
                .metadata
                .insert("prefab.source_id".to_owned(), source_id.0.to_string());
            entity.metadata.insert(
                "prefab.root".to_owned(),
                (entity_id == &instance.instance_root).to_string(),
            );
        }

        let outer = Prefab::from_scene(
            &scene,
            SceneEntityId(1),
            AssetId::from_seed("root.prefab"),
            "Root",
        )
        .unwrap();
        assert_eq!(
            outer.nested_sources(),
            vec!["assets/bar.prefab".to_owned()],
            "外层必须知道自己嵌了谁"
        );
        let dir = temp_prefab_dir("nested-roundtrip");
        outer.save(dir.join("root.prefab")).unwrap();
        let loaded = Prefab::load(dir.join("root.prefab")).unwrap();
        assert_eq!(loaded, outer, "文本往返必须逐字保真");

        let mut target = Scene::new("target");
        let copy = loaded
            .instantiate(&mut target, None, &BTreeMap::new())
            .unwrap();
        let inner_nodes: Vec<_> = target
            .entities
            .values()
            .filter(|entity| entity.metadata.contains_key(PREFAB_SOURCE_META_KEY))
            .cloned()
            .collect();
        assert_eq!(inner_nodes.len(), 1, "内层节点要带着标记：{target:?}");
        let metadata = &inner_nodes[0].metadata;
        assert_eq!(metadata[PREFAB_SOURCE_META_KEY], "assets/bar.prefab");
        assert_eq!(metadata["prefab.source_id"], "1", "指向内层文件的实体编号");
        assert_eq!(metadata["prefab.root"], "true");
        assert!(
            target.entities[&copy.instance_root]
                .children
                .contains(&inner_nodes[0].id),
            "内层实例的根仍然是外层根的子节点"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    /// 嵌套覆盖记录：键指向**外层文件**的实体编号，且只能落在带内层标记的节点上。
    ///
    /// 它和变体覆盖回答的是两个问题（外层 vs 内层 / 变体 vs 基础），因此校验必须能
    /// 把"写到外层自己的节点上"这种会误导同步的记录挡在入口。
    #[test]
    fn nested_override_records_round_trip_and_reject_non_nested_targets() {
        let mut inner_scene = Scene::new("inner");
        inner_scene
            .add_entity(SceneEntityId(1), "Bar", None)
            .unwrap();
        inner_scene
            .entities
            .get_mut(&SceneEntityId(1))
            .unwrap()
            .components
            .insert(
                "Sprite".to_owned(),
                "texture=a.png;size=16x16;tint=1.000,1.000,1.000,1.000;sorting_layer=world"
                    .to_owned(),
            );
        let inner = Prefab::from_scene(
            &inner_scene,
            SceneEntityId(1),
            AssetId::from_seed("bar.prefab"),
            "Bar",
        )
        .unwrap();
        let mut scene = Scene::new("outer");
        scene.add_entity(SceneEntityId(1), "Root", None).unwrap();
        let instance = inner
            .instantiate(&mut scene, Some(SceneEntityId(1)), &BTreeMap::new())
            .unwrap();
        for (source_id, entity_id) in &instance.source_to_instance {
            let entity = scene.entities.get_mut(entity_id).unwrap();
            entity.metadata.insert(
                PREFAB_SOURCE_META_KEY.to_owned(),
                "assets/prefabs/bar.prefab".to_owned(),
            );
            entity
                .metadata
                .insert("prefab.source_id".to_owned(), source_id.0.to_string());
            entity.metadata.insert(
                "prefab.root".to_owned(),
                (entity_id == &instance.instance_root).to_string(),
            );
        }
        let mut outer = Prefab::from_scene(
            &scene,
            SceneEntityId(1),
            AssetId::from_seed("outer.prefab"),
            "Outer",
        )
        .unwrap();
        outer
            .set_nested_stamp("assets/prefabs/bar.prefab", Some("len:7"))
            .unwrap();
        let records = BTreeMap::from([
            (
                (instance.instance_root, "name".to_owned()),
                "BarOverride".to_owned(),
            ),
            (
                (instance.instance_root, "property:Sprite.tint".to_owned()),
                "0.500,1.000,1.000,1.000".to_owned(),
            ),
        ]);
        outer.set_nested_overrides(&records).unwrap();
        assert_eq!(outer.nested_overrides().unwrap(), records);
        assert_eq!(
            outer.entities[&instance.instance_root].components["Sprite"],
            "texture=a.png;size=16x16;tint=1.000,1.000,1.000,1.000;sorting_layer=world",
            "记录不改写快照"
        );

        let text = outer.to_text();
        let loaded = Prefab::from_text(&text).unwrap();
        assert_eq!(loaded, outer, "嵌套覆盖记录必须逐字往返");
        assert_eq!(loaded.nested_stamps().len(), 1);

        // 外层自己的节点（没有内层标记）不能记嵌套覆盖：那会让同步把外层节点当内层。
        let mut wrong_target = outer.clone();
        wrong_target
            .set_nested_overrides(&BTreeMap::from([(
                (outer.root, "name".to_owned()),
                "x".to_owned(),
            )]))
            .expect_err("非嵌套节点必须被拒绝");
        assert!(matches!(
            wrong_target.set_nested_overrides(&BTreeMap::from([(
                (SceneEntityId(99), "name".to_owned()),
                "x".to_owned()
            )])),
            Err(PrefabError::MissingEntity(SceneEntityId(99)))
        ));
        // 载入路径同样拒绝：目标不是嵌套节点、字段语法不合法。
        for key in [
            "prefab.nested_override.1.name",
            "prefab.nested_override.2.bogus",
            "prefab.nested_override.2.metadata:prefab.source",
        ] {
            let mut probe = outer.clone();
            probe.metadata.insert(key.to_owned(), "x".to_owned());
            assert!(
                probe.validate().is_err(),
                "坏嵌套覆盖记录必须被 validate 拒绝：{key}"
            );
        }
    }

    /// 变体谱系与覆盖记录必须**只活在文件级元数据里**：快照仍是运行时唯一事实源，
    /// 实例化不自动叠加它们（编辑器在做显式覆盖时才用）。
    #[test]
    fn variant_lineage_and_overrides_round_trip_without_touching_snapshot() {
        let s = scene();
        let base = Prefab::from_scene(
            &s,
            SceneEntityId(1),
            AssetId::from_seed("base.prefab"),
            "Base",
        )
        .unwrap();
        let mut variant = base.clone();
        variant
            .set_variant_base(Some("assets/prefabs/base.prefab"))
            .unwrap();
        variant.set_variant_base_stamp(Some("len:1")).unwrap();
        variant
            .set_nested_stamp("assets/prefabs/child.prefab", Some("len:2"))
            .unwrap();
        let overrides = BTreeMap::from([(
            (SceneEntityId(2), "component:Transform".to_owned()),
            "x=9".to_owned(),
        )]);
        variant.set_overrides(&overrides).unwrap();
        assert_eq!(variant.overrides().unwrap(), overrides);
        assert_eq!(
            variant.entities[&SceneEntityId(2)].components["Transform"],
            "x=1",
            "覆盖记录不能改写快照"
        );

        let text = variant.to_text();
        assert!(
            !text.lines().any(|line| line.ends_with('\t')),
            "谱系记录不能引入行尾制表符：{text}"
        );
        // 实体记录条数必须与快照实体数一致：覆盖记录不是实体。
        let entity_lines = text
            .lines()
            .filter(|line| line.starts_with("entity\t"))
            .count();
        assert_eq!(entity_lines, variant.entities.len());

        let loaded = Prefab::from_text(&text).unwrap();
        assert_eq!(loaded, variant, "谱系与覆盖必须逐字往返");
        assert_eq!(loaded.version, CURRENT_PREFAB_VERSION);
        assert_eq!(loaded.variant_base(), Some("assets/prefabs/base.prefab"));
        assert_eq!(loaded.variant_base_stamp(), Some("len:1"));
        assert_eq!(
            loaded.nested_stamp("assets/prefabs/child.prefab"),
            Some("len:2")
        );
        assert_eq!(
            loaded.nested_stamps(),
            BTreeMap::from([("assets/prefabs/child.prefab".to_owned(), "len:2".to_owned())])
        );

        // 快照路径：不传覆盖 → 读到基础值。
        let mut plain = Scene::new("plain");
        let copy = loaded
            .instantiate(&mut plain, None, &BTreeMap::new())
            .unwrap();
        assert_eq!(
            plain.entities[&copy.source_to_instance[&SceneEntityId(2)]].components["Transform"],
            "x=1"
        );
        // 显式叠加：同一套 apply_overrides 让覆盖落到新快照的实例上。
        let mut overridden = Scene::new("overridden");
        let copy = loaded
            .instantiate(&mut overridden, None, &loaded.overrides().unwrap())
            .unwrap();
        assert_eq!(
            overridden.entities[&copy.source_to_instance[&SceneEntityId(2)]].components["Transform"],
            "x=9"
        );

        // 换基础会作废旧签名：不能拿上一份基础的 stamp 去同步新基础。
        variant
            .set_variant_base(Some("assets/prefabs/other.prefab"))
            .unwrap();
        assert_eq!(variant.variant_base_stamp(), None);
    }

    #[test]
    fn invalid_variant_records_are_rejected() {
        let s = scene();
        let base = Prefab::from_scene(
            &s,
            SceneEntityId(1),
            AssetId::from_seed("base.prefab"),
            "Base",
        )
        .unwrap();

        // 基础路径必须是工程内相对 .prefab：绝对路径、`..`、别的扩展名都拒。
        for bad in [
            "/abs/base.prefab",
            "assets/../base.prefab",
            "assets/base.scene",
        ] {
            assert!(
                base.clone().set_variant_base(Some(bad)).is_err(),
                "基础路径应被拒绝：{bad}"
            );
        }
        // 只有基础、没有签名 → 无法判断"基础动没动过"。
        let mut no_stamp = base.clone();
        no_stamp
            .set_variant_base(Some("assets/prefabs/base.prefab"))
            .unwrap();
        assert!(matches!(
            no_stamp.validate(),
            Err(PrefabError::InvalidVariant(_))
        ));
        // 只有签名、没有基础同样不自洽。
        let mut orphan_stamp = base.clone();
        orphan_stamp.set_variant_base_stamp(Some("len:1")).unwrap();
        assert!(matches!(
            orphan_stamp.validate(),
            Err(PrefabError::InvalidVariant(_))
        ));

        // 覆盖记录的五类坏数据：未知实体、坏字段语法、组件不存在、覆盖身份键、拆不开的键。
        let mut bad = base.clone();
        bad.set_variant_base(Some("assets/prefabs/base.prefab"))
            .unwrap();
        bad.set_variant_base_stamp(Some("len:1")).unwrap();
        for (key, reason) in [
            ("prefab.override.99.name", "未知实体"),
            ("prefab.override.1.bogus", "坏字段语法"),
            ("prefab.override.1.property:Sprite.tint[1]", "组件不存在"),
            ("prefab.override.1.metadata:prefab.source", "覆盖身份键"),
            ("prefab.override.1", "拆不开的键"),
        ] {
            let mut probe = bad.clone();
            probe.metadata.insert(key.to_owned(), "x".to_owned());
            assert!(
                probe.validate().is_err(),
                "坏记录必须被 validate 拒绝（{reason}）：{key}"
            );
        }

        // 载入路径也走同一道校验：文件里的坏记录不能读进内存。
        let text = base.to_text().replace(
            "name=Base\n",
            "name=Base\nmeta\tprefab.override.1.bogus\tx\n",
        );
        assert!(matches!(
            Prefab::from_text(&text),
            Err(PrefabError::InvalidOverride(_))
        ));
    }

    fn temp_prefab_dir(label: &str) -> PathBuf {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("rustlua-prefab-{label}-{n}-{}", std::process::id()));
        fs::create_dir_all(&dir).expect("创建临时目录");
        dir
    }
}
