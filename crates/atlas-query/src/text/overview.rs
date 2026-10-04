use crate::queries::OverviewAnswer;
use crate::text::{RenderText, TextOut, entity_line};

const PACKAGES_SHOWN: usize = 15;
const APPLICATIONS_SHOWN: usize = 10;

impl RenderText for OverviewAnswer {
  fn render_text(&self, out: &mut TextOut) {
    let files: usize = self.repos.iter().map(|repo| repo.files).sum();
    out.line(0, format!("project {}: {} repos, {files} files", self.project, self.repos.len()));
    out.line(0, "repos");
    for repo in &self.repos {
      let languages: Vec<String> = repo.languages.iter().map(|(language, count)| format!("{language} {count}")).collect();
      let problems = match repo.problem_files {
        0 => String::new(),
        count => format!(", {count} parse problems"),
      };
      out.line(1, format!("{}: {} files ({}){problems}", repo.repo, repo.files, languages.join(", ")));
    }
    out.line(0, format!("packages ({})", self.packages.len()));
    for package in self.packages.iter().take(PACKAGES_SHOWN) {
      let internal = match package.in_project_dependencies.is_empty() {
        true => String::new(),
        false => format!(" -> {}", package.in_project_dependencies.join(", ")),
      };
      let dependents = match package.dependents.is_empty() {
        true => String::new(),
        false => format!("; used by {}", package.dependents.join(", ")),
      };
      out.line(
        1,
        format!(
          "{} [{}] {}: {} external deps{internal}{dependents}",
          package.package.short, package.ecosystem, package.repo, package.external_dependencies
        ),
      );
    }
    if self.packages.len() > PACKAGES_SHOWN {
      out.line(1, format!("{} more packages (see the JSON output)", self.packages.len() - PACKAGES_SHOWN));
    }
    if !self.applications.is_empty() {
      out.line(0, format!("applications ({})", self.applications.len()));
      for application in self.applications.iter().take(APPLICATIONS_SHOWN) {
        let entrypoints: Vec<String> = application.entrypoints.iter().map(entity_line).collect();
        out.line(
          1,
          format!(
            "{} {} {} (basis {}{})",
            application.application.short,
            application.application.repo(),
            application.certainty.tag,
            application.basis,
            application.basis_source.as_ref().map(|attr| format!(" {}", attr.tag())).unwrap_or_default()
          ),
        );
        if !entrypoints.is_empty() {
          out.line(2, format!("entrypoints: {}", entrypoints.join("; ")));
        }
      }
    }
    if self.applications.len() > APPLICATIONS_SHOWN {
      out.line(1, format!("{} more applications (see the JSON output)", self.applications.len() - APPLICATIONS_SHOWN));
    }
    if !self.top_modules.is_empty() {
      out.line(0, "top modules by fan-in");
      for module in &self.top_modules {
        out.line(1, format!("{} ({} importers)", module.file, module.importers));
      }
    }
    if !self.apis.is_empty() {
      out.line(0, "APIs");
      for api in &self.apis {
        let schemes = match api.auth_schemes.is_empty() {
          true => "none detected".to_owned(),
          false => api.auth_schemes.join(", "),
        };
        out.line(1, format!("{}: {} endpoints, auth {schemes}", api.repo, api.endpoints));
      }
    }
    if !self.events.is_empty() {
      out.line(0, "events");
      for event in &self.events {
        out.line(1, format!("{}: {} publishers, {} subscribers", event.event, event.publishers, event.subscribers));
      }
    }
    if !self.patterns.is_empty() {
      out.line(0, "patterns");
      for pattern in &self.patterns {
        out.line(1, pattern);
      }
    }
    out.line(0, "health");
    for line in &self.health {
      out.line(1, line);
    }
  }
}
