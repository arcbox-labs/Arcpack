/// Layer —— 步骤间的连接机制
///
/// 对齐 railpack `core/plan/layer.go`。
/// 三种互斥引用方式：
/// - step: 引用另一个步骤的输出
/// - image: 引用外部 Docker 镜像
/// - local: 引用本地构建上下文（源码目录）
///
/// Filter 通过 serde flatten 内嵌，include/exclude 提升到顶层 JSON 字段。
use serde::{Deserialize, Deserializer, Serialize};

use super::filter::Filter;
use super::spread::Spreadable;

#[derive(Debug, Clone, PartialEq, Default, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Layer {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub step: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub local: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub spread: Option<bool>,
    #[serde(flatten)]
    pub filter: Filter,
}

#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LayerSerde {
    #[serde(default)]
    image: Option<String>,
    #[serde(default)]
    step: Option<String>,
    #[serde(default)]
    local: Option<bool>,
    #[serde(default)]
    spread: Option<bool>,
    #[serde(flatten)]
    filter: Filter,
}

impl<'de> Deserialize<'de> for Layer {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = serde_json::Value::deserialize(deserializer)?;

        // 兼容 railpack 配置：Layer 允许使用 "..." 作为 spread 占位符
        if let Some(s) = value.as_str() {
            if s == "..." {
                return Ok(Layer {
                    spread: Some(true),
                    ..Default::default()
                });
            }
            return Err(serde::de::Error::custom(format!(
                "无效的 Layer 字符串表示: {s}"
            )));
        }

        if value.is_object() {
            let raw: LayerSerde =
                serde_json::from_value(value).map_err(serde::de::Error::custom)?;
            return Ok(Layer {
                image: raw.image,
                step: raw.step,
                local: raw.local,
                spread: raw.spread,
                filter: raw.filter,
            });
        }

        Err(serde::de::Error::custom("Layer 必须是 JSON 对象或 \"...\""))
    }
}

impl Layer {
    /// 创建引用另一个步骤的 Layer
    pub fn new_step_layer(name: impl Into<String>, filter: Option<Filter>) -> Self {
        Self {
            step: Some(name.into()),
            filter: filter.unwrap_or_default(),
            ..Default::default()
        }
    }

    /// 创建引用外部 Docker 镜像的 Layer
    pub fn new_image_layer(image: impl Into<String>, filter: Option<Filter>) -> Self {
        Self {
            image: Some(image.into()),
            filter: filter.unwrap_or_default(),
            ..Default::default()
        }
    }

    /// 创建引用本地构建上下文的 Layer
    pub fn new_local_layer() -> Self {
        Self {
            local: Some(true),
            filter: Filter::include_only(vec![".".to_string()]),
            ..Default::default()
        }
    }

    /// 检查 Layer 是否为空（无 step/image/local/spread）
    pub fn is_empty(&self) -> bool {
        self.step.is_none() && self.image.is_none() && self.local.is_none() && self.spread.is_none()
    }

    /// 显示名称（用于日志）
    pub fn display_name(&self) -> String {
        if let Some(ref step) = self.step {
            format!("step:{}", step)
        } else if let Some(ref image) = self.image {
            format!("image:{}", image)
        } else if self.local == Some(true) {
            "local".to_string()
        } else if self.spread == Some(true) {
            "...".to_string()
        } else {
            "empty".to_string()
        }
    }
}

/// Layer 的 Spreadable 实现
impl Spreadable for Layer {
    fn is_spread(&self) -> bool {
        self.spread == Some(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_step_layer_sets_step_field() {
        let layer = Layer::new_step_layer("install", None);
        assert_eq!(layer.step, Some("install".to_string()));
        assert!(layer.image.is_none());
        assert!(layer.local.is_none());
    }

    #[test]
    fn test_new_step_layer_with_filter() {
        let filter = Filter::include_only(vec![".".to_string()]);
        let layer = Layer::new_step_layer("build", Some(filter.clone()));
        assert_eq!(layer.step, Some("build".to_string()));
        assert_eq!(layer.filter, filter);
    }

    #[test]
    fn test_new_image_layer_sets_image_field() {
        let layer = Layer::new_image_layer("ghcr.io/railwayapp/railpack-builder:latest", None);
        assert_eq!(
            layer.image,
            Some("ghcr.io/railwayapp/railpack-builder:latest".to_string())
        );
        assert!(layer.step.is_none());
    }

    #[test]
    fn test_new_local_layer_sets_local_and_include() {
        let layer = Layer::new_local_layer();
        assert_eq!(layer.local, Some(true));
        assert_eq!(layer.filter.include, vec!["."]);
    }

    #[test]
    fn test_layer_json_roundtrip_step() {
        let layer =
            Layer::new_step_layer("build", Some(Filter::include_only(vec![".".to_string()])));
        let json = serde_json::to_string(&layer).unwrap();
        let parsed: Layer = serde_json::from_str(&json).unwrap();
        assert_eq!(layer, parsed);
        // 验证 filter 字段被 flatten 到顶层
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert!(value.get("include").is_some());
        assert!(value.get("filter").is_none());
    }

    #[test]
    fn test_layer_json_roundtrip_image() {
        let layer = Layer::new_image_layer("ubuntu:22.04", None);
        let json = serde_json::to_string(&layer).unwrap();
        let parsed: Layer = serde_json::from_str(&json).unwrap();
        assert_eq!(layer, parsed);
    }

    #[test]
    fn test_layer_is_empty() {
        assert!(Layer::default().is_empty());
        assert!(!Layer::new_step_layer("build", None).is_empty());
    }

    #[test]
    fn test_layer_display_name() {
        assert_eq!(
            Layer::new_step_layer("install", None).display_name(),
            "step:install"
        );
        assert_eq!(
            Layer::new_image_layer("ubuntu:22.04", None).display_name(),
            "image:ubuntu:22.04"
        );
        assert_eq!(Layer::new_local_layer().display_name(), "local");
    }

    #[test]
    fn test_layer_deserialize_spread_string() {
        let layer: Layer = serde_json::from_str(r#""...""#).unwrap();
        assert_eq!(layer.spread, Some(true));
        assert!(layer.step.is_none());
        assert!(layer.image.is_none());
        assert!(layer.local.is_none());
    }
}
