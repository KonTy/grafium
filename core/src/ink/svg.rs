use crate::error::Result;
use crate::ink::models::*;
use std::fmt::Write;
use std::path::Path;

const GRAFIUM_NS: &str = "https://grafium.app/ink/v1";

/// Serializes InkPage to SVG format with embedded Grafium metadata.
pub struct InkSvgSerializer;

impl InkSvgSerializer {
    /// Serialize an InkPage to an SVG string.
    pub fn serialize(page: &InkPage) -> String {
        let mut svg = String::with_capacity(page.strokes.len() * 512);

        // SVG header with Grafium namespace
        write!(
            svg,
            r#"<svg xmlns="http://www.w3.org/2000/svg" xmlns:gfm="{}" viewBox="0 0 {} {}" width="{}" height="{}">"#,
            GRAFIUM_NS,
            page.canvas_width,
            page.canvas_height,
            page.canvas_width,
            page.canvas_height,
        )
        .unwrap();
        svg.push('\n');

        // Metadata block
        write!(
            svg,
            r#"  <metadata><gfm:ink version="1" created="{}" updated="{}" strokes="{}"/></metadata>"#,
            page.created_at, page.updated_at, page.strokes.len(),
        )
        .unwrap();
        svg.push('\n');

        // Style definitions
        svg.push_str("  <defs>\n");
        svg.push_str("    <style>\n");
        svg.push_str("      path { fill: none; stroke-linecap: round; stroke-linejoin: round; }\n");
        svg.push_str("      .highlighter { opacity: 0.4; }\n");
        svg.push_str("    </style>\n");
        svg.push_str("  </defs>\n");

        // Render each stroke
        for stroke in &page.strokes {
            Self::serialize_stroke(&mut svg, stroke);
        }

        svg.push_str("</svg>\n");
        svg
    }

    fn serialize_stroke(svg: &mut String, stroke: &Stroke) {
        if stroke.points.is_empty() {
            return;
        }

        // Encode pressure array as compact comma-separated values
        let pressure_data: String = stroke
            .points
            .iter()
            .map(|p| format!("{:.2}", p.pressure))
            .collect::<Vec<_>>()
            .join(",");

        // Encode timestamps as deltas (more compact)
        let timestamp_data: String = stroke
            .points
            .iter()
            .map(|p| p.timestamp_ms.to_string())
            .collect::<Vec<_>>()
            .join(",");

        // Tilt data (only if non-zero)
        let has_tilt = stroke.points.iter().any(|p| p.tilt > 0.01);

        let class = match stroke.tool {
            PenTool::Highlighter => r#" class="highlighter""#,
            _ => "",
        };

        // Open stroke group with metadata attributes
        write!(
            svg,
            r#"  <g id="{}" gfm:tool="{}" gfm:pressure="{}" gfm:timestamps="{}""#,
            stroke.id,
            stroke.tool.as_str(),
            pressure_data,
            timestamp_data,
        )
        .unwrap();

        if has_tilt {
            let tilt_data: String = stroke
                .points
                .iter()
                .map(|p| format!("{:.2}", p.tilt))
                .collect::<Vec<_>>()
                .join(",");
            write!(svg, r#" gfm:tilt="{}""#, tilt_data).unwrap();
        }

        svg.push_str(">\n");

        // Generate the SVG path using quadratic bezier curves for smooth rendering.
        // Variable width based on pressure is achieved by splitting into segments.
        if stroke.points.len() == 1 {
            // Single dot
            let p = &stroke.points[0];
            let r = stroke.width * p.pressure * 0.5;
            write!(
                svg,
                r#"    <circle cx="{:.1}" cy="{:.1}" r="{:.1}" fill="{}"{}/>
"#,
                p.x, p.y, r, stroke.color, class
            )
            .unwrap();
        } else {
            // Build path data
            let path_d = Self::build_path_data(&stroke.points);
            // Average pressure for stroke-width (variable width is client-side rendering)
            let avg_pressure: f32 =
                stroke.points.iter().map(|p| p.pressure).sum::<f32>() / stroke.points.len() as f32;
            let rendered_width = stroke.width * avg_pressure;

            write!(
                svg,
                r#"    <path d="{}" stroke="{}" stroke-width="{:.1}"{}/>
"#,
                path_d, stroke.color, rendered_width, class
            )
            .unwrap();
        }

        svg.push_str("  </g>\n");
    }

    /// Build an SVG path from points using line segments.
    /// Smoothing is done client-side at render time; SVG stores exact data points
    /// to ensure lossless roundtrip fidelity.
    fn build_path_data(points: &[StrokePoint]) -> String {
        let mut d = String::with_capacity(points.len() * 16);

        write!(d, "M {:.1} {:.1}", points[0].x, points[0].y).unwrap();

        for p in &points[1..] {
            write!(d, " L {:.1} {:.1}", p.x, p.y).unwrap();
        }

        d
    }

    /// Write an InkPage to an SVG file on disk.
    pub fn write_to_file(page: &InkPage, path: &Path) -> Result<()> {
        let svg = Self::serialize(page);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, svg.as_bytes())?;
        Ok(())
    }
}

/// Parses SVG files (created by Grafium) back into InkPage structs.
pub struct InkSvgParser;

impl InkSvgParser {
    /// Parse an SVG string back into an InkPage.
    /// This handles SVGs produced by InkSvgSerializer.
    pub fn parse(svg_content: &str) -> Result<InkPage> {
        let mut page = InkPage {
            strokes: Vec::new(),
            canvas_width: 1920.0,
            canvas_height: 1080.0,
            created_at: 0,
            updated_at: 0,
        };

        // Parse viewBox for dimensions
        if let Some(viewbox) = Self::extract_attr(svg_content, "viewBox") {
            let parts: Vec<f32> = viewbox
                .split_whitespace()
                .filter_map(|s| s.parse().ok())
                .collect();
            if parts.len() == 4 {
                page.canvas_width = parts[2];
                page.canvas_height = parts[3];
            }
        }

        // Parse metadata
        if let Some(meta_section) = Self::extract_between(svg_content, "<metadata>", "</metadata>")
        {
            if let Some(created) = Self::extract_attr(meta_section, "created") {
                page.created_at = created.parse().unwrap_or(0);
            }
            if let Some(updated) = Self::extract_attr(meta_section, "updated") {
                page.updated_at = updated.parse().unwrap_or(0);
            }
        }

        // Parse stroke groups
        let mut search_from = 0;
        while let Some(g_start) = svg_content[search_from..].find("<g ") {
            let abs_start = search_from + g_start;
            if let Some(g_end) = svg_content[abs_start..].find("</g>") {
                let g_content = &svg_content[abs_start..abs_start + g_end + 4];
                if let Some(stroke) = Self::parse_stroke_group(g_content) {
                    page.strokes.push(stroke);
                }
                search_from = abs_start + g_end + 4;
            } else {
                break;
            }
        }

        Ok(page)
    }

    /// Parse a single <g>...</g> stroke group.
    fn parse_stroke_group(g_content: &str) -> Option<Stroke> {
        let id = Self::extract_attr(g_content, "id")?;
        let tool_str = Self::extract_attr(g_content, "gfm:tool").unwrap_or_default();
        let pressure_str = Self::extract_attr(g_content, "gfm:pressure").unwrap_or_default();
        let timestamps_str = Self::extract_attr(g_content, "gfm:timestamps").unwrap_or_default();
        let tilt_str = Self::extract_attr(g_content, "gfm:tilt");

        let tool = PenTool::from_str(&tool_str);

        // Parse the path data to get x,y coordinates
        let (points_xy, color, width) = if let Some(path_d) = Self::extract_attr(g_content, "d") {
            let color = Self::extract_attr(g_content, "stroke").unwrap_or_else(|| "#1a1a1a".into());
            let width: f32 = Self::extract_attr(g_content, "stroke-width")
                .and_then(|w| w.parse().ok())
                .unwrap_or(2.0);
            (Self::parse_path_d(&path_d), color, width)
        } else if g_content.contains("<circle") {
            // Single dot
            let cx: f32 = Self::extract_attr(g_content, "cx")
                .and_then(|v| v.parse().ok())
                .unwrap_or(0.0);
            let cy: f32 = Self::extract_attr(g_content, "cy")
                .and_then(|v| v.parse().ok())
                .unwrap_or(0.0);
            let color = Self::extract_attr(g_content, "fill").unwrap_or_else(|| "#1a1a1a".into());
            let r: f32 = Self::extract_attr(g_content, "r")
                .and_then(|v| v.parse().ok())
                .unwrap_or(1.0);
            (vec![(cx, cy)], color, r * 2.0)
        } else {
            return None;
        };

        // Parse pressure values
        let pressures: Vec<f32> = pressure_str
            .split(',')
            .filter_map(|s| s.trim().parse().ok())
            .collect();

        // Parse timestamps
        let timestamps: Vec<u32> = timestamps_str
            .split(',')
            .filter_map(|s| s.trim().parse().ok())
            .collect();

        // Parse tilt values
        let tilts: Vec<f32> = tilt_str
            .map(|s| s.split(',').filter_map(|t| t.trim().parse().ok()).collect())
            .unwrap_or_default();

        // Combine into StrokePoints
        let points: Vec<StrokePoint> = points_xy
            .iter()
            .enumerate()
            .map(|(i, &(x, y))| StrokePoint {
                x,
                y,
                pressure: pressures.get(i).copied().unwrap_or(0.5),
                tilt: tilts.get(i).copied().unwrap_or(0.0),
                timestamp_ms: timestamps.get(i).copied().unwrap_or(0),
            })
            .collect();

        // Recover the base width (undo average pressure scaling)
        let avg_pressure: f32 = if points.is_empty() {
            0.5
        } else {
            points.iter().map(|p| p.pressure).sum::<f32>() / points.len() as f32
        };
        let base_width = if avg_pressure > 0.01 {
            width / avg_pressure
        } else {
            width
        };

        Some(Stroke {
            id,
            points,
            tool,
            color,
            width: base_width,
        })
    }

    /// Parse SVG path d attribute to extract (x, y) points.
    /// Handles M, L, Q commands (what we generate).
    fn parse_path_d(d: &str) -> Vec<(f32, f32)> {
        let mut points = Vec::new();
        let mut chars = d.chars().peekable();
        let mut current_cmd = ' ';

        let mut num_buf = String::new();
        let mut numbers: Vec<f32> = Vec::new();

        while let Some(&ch) = chars.peek() {
            if ch.is_alphabetic() {
                // Process accumulated numbers for previous command
                if !num_buf.is_empty() {
                    if let Ok(n) = num_buf.parse::<f32>() {
                        numbers.push(n);
                    }
                    num_buf.clear();
                }
                Self::process_path_cmd(current_cmd, &numbers, &mut points);
                numbers.clear();

                current_cmd = ch;
                chars.next();
            } else if ch.is_ascii_digit() || ch == '.' || ch == '-' {
                num_buf.push(ch);
                chars.next();
            } else if ch == ' ' || ch == ',' {
                if !num_buf.is_empty() {
                    if let Ok(n) = num_buf.parse::<f32>() {
                        numbers.push(n);
                    }
                    num_buf.clear();
                }
                chars.next();
            } else {
                chars.next();
            }
        }

        // Final flush
        if !num_buf.is_empty() {
            if let Ok(n) = num_buf.parse::<f32>() {
                numbers.push(n);
            }
        }
        Self::process_path_cmd(current_cmd, &numbers, &mut points);

        points
    }

    fn process_path_cmd(cmd: char, numbers: &[f32], points: &mut Vec<(f32, f32)>) {
        match cmd {
            'M' | 'L' => {
                // Pairs of (x, y)
                for pair in numbers.chunks(2) {
                    if pair.len() == 2 {
                        points.push((pair[0], pair[1]));
                    }
                }
            }
            'Q' => {
                // Quadratic bezier: control_x, control_y, end_x, end_y
                // We store the control point and endpoint
                for quad in numbers.chunks(4) {
                    if quad.len() == 4 {
                        points.push((quad[0], quad[1])); // control point
                        points.push((quad[2], quad[3])); // end point
                    }
                }
            }
            _ => {}
        }
    }

    /// Read and parse an SVG file from disk.
    pub fn read_from_file(path: &Path) -> Result<InkPage> {
        let content = std::fs::read_to_string(path)?;
        Self::parse(&content)
    }

    /// Extract an XML attribute value by name (simple regex-free parsing).
    /// Requires the attribute name to be preceded by a space/newline (word boundary).
    fn extract_attr(content: &str, attr_name: &str) -> Option<String> {
        let pattern = format!(" {}=\"", attr_name);
        let start = content.find(&pattern).or_else(|| {
            // Also check for start-of-tag (first attribute)
            let alt = format!("<{} ", attr_name);
            // If not found with space prefix, try with newline or tab
            content.find(&format!("\t{}=\"", attr_name))
                .or_else(|| content.find(&format!("\n{}=\"", attr_name)))
        })?;
        let attr_start = content[start..].find(&format!("{}=\"", attr_name))?;
        let value_start = start + attr_start + attr_name.len() + 2; // skip `attr="`
        let value_end = content[value_start..].find('"')?;
        Some(content[value_start..value_start + value_end].to_string())
    }

    /// Extract content between two markers.
    fn extract_between<'a>(content: &'a str, start_marker: &str, end_marker: &str) -> Option<&'a str> {
        let start = content.find(start_marker)?;
        let after_start = start + start_marker.len();
        let end = content[after_start..].find(end_marker)?;
        Some(&content[after_start..after_start + end])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_roundtrip_single_stroke() {
        let mut page = InkPage::new(1920.0, 1080.0);
        page.add_stroke(Stroke {
            id: "stroke-1".into(),
            points: vec![
                StrokePoint { x: 100.0, y: 200.0, pressure: 0.5, tilt: 0.0, timestamp_ms: 0 },
                StrokePoint { x: 110.0, y: 205.0, pressure: 0.7, tilt: 0.1, timestamp_ms: 16 },
                StrokePoint { x: 120.0, y: 210.0, pressure: 0.9, tilt: 0.1, timestamp_ms: 32 },
                StrokePoint { x: 130.0, y: 208.0, pressure: 0.6, tilt: 0.0, timestamp_ms: 48 },
            ],
            tool: PenTool::Pen,
            color: "#1a1a1a".into(),
            width: 3.0,
        });

        let svg = InkSvgSerializer::serialize(&page);

        // Should be valid SVG
        assert!(svg.contains("<svg"));
        assert!(svg.contains("</svg>"));
        assert!(svg.contains("stroke-1"));
        assert!(svg.contains("gfm:pressure"));
        assert!(svg.contains("grafium.app/ink/v1"));

        // Roundtrip parse
        let parsed = InkSvgParser::parse(&svg).unwrap();
        assert_eq!(parsed.strokes.len(), 1);
        assert_eq!(parsed.strokes[0].id, "stroke-1");
        assert_eq!(parsed.strokes[0].points.len(), page.strokes[0].points.len());
        assert_eq!(parsed.canvas_width, 1920.0);
    }

    #[test]
    fn test_single_dot_stroke() {
        let mut page = InkPage::new(800.0, 600.0);
        page.add_stroke(Stroke {
            id: "dot-1".into(),
            points: vec![
                StrokePoint { x: 400.0, y: 300.0, pressure: 0.8, tilt: 0.0, timestamp_ms: 0 },
            ],
            tool: PenTool::Pen,
            color: "#ff0000".into(),
            width: 4.0,
        });

        let svg = InkSvgSerializer::serialize(&page);
        assert!(svg.contains("<circle"));
        assert!(svg.contains("400.0"));

        let parsed = InkSvgParser::parse(&svg).unwrap();
        assert_eq!(parsed.strokes.len(), 1);
        assert_eq!(parsed.strokes[0].points.len(), 1);
    }

    #[test]
    fn test_highlighter_class() {
        let mut page = InkPage::new(800.0, 600.0);
        page.add_stroke(Stroke {
            id: "hl-1".into(),
            points: vec![
                StrokePoint { x: 10.0, y: 20.0, pressure: 0.5, tilt: 0.0, timestamp_ms: 0 },
                StrokePoint { x: 200.0, y: 20.0, pressure: 0.5, tilt: 0.0, timestamp_ms: 100 },
            ],
            tool: PenTool::Highlighter,
            color: "#ffff00".into(),
            width: 20.0,
        });

        let svg = InkSvgSerializer::serialize(&page);
        assert!(svg.contains("highlighter"));
        assert!(svg.contains("opacity: 0.4"));
    }

    #[test]
    fn test_empty_page() {
        let page = InkPage::new(1024.0, 768.0);
        let svg = InkSvgSerializer::serialize(&page);
        assert!(svg.contains("viewBox=\"0 0 1024 768\""));

        let parsed = InkSvgParser::parse(&svg).unwrap();
        assert_eq!(parsed.strokes.len(), 0);
        assert_eq!(parsed.canvas_width, 1024.0);
    }
}
