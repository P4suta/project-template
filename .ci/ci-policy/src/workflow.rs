use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::str::FromStr;
use yaml_rust2::{Yaml, YamlLoader};

use crate::immutable_reference;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Repository {
    pub repository: String,
    pub workflows: Vec<Source>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Source {
    pub name: String,
    pub text: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Rule {
    ExplicitPermissions,
    ExcessivePermissions,
    JobTimeout,
    ImmutableAction,
    CheckoutCredentials,
    PullRequestConcurrency,
    NoOpRequired,
}

#[derive(Debug, Serialize)]
pub struct Finding {
    pub repository: String,
    pub workflow: String,
    pub job: Option<String>,
    pub rule: Rule,
    pub detail: String,
}

#[derive(Debug, Serialize)]
pub struct WorkflowSummary {
    pub repository: String,
    pub workflow: String,
    pub name: String,
    pub events: Vec<String>,
    pub jobs: Vec<String>,
    pub actions: Vec<String>,
    pub commands: Vec<String>,
    pub findings: Vec<Finding>,
}

pub fn parse(text: &str) -> Result<Yaml> {
    let mut documents = YamlLoader::load_from_str(text).context("invalid workflow YAML")?;
    ensure!(
        documents.len() == 1,
        "a workflow must contain exactly one YAML document"
    );
    let document = documents.remove(0);
    ensure!(
        document.as_hash().is_some(),
        "workflow root must be a mapping"
    );
    Ok(document)
}

fn project_reference(mapping: &yaml_edit::Mapping, semantic: &mut Yaml) -> Result<()> {
    let Yaml::Hash(fields) = semantic else {
        anyhow::bail!("reference owner must be a mapping")
    };
    let Some(reference) = fields.get_mut(&Yaml::String("uses".to_owned())) else {
        return Ok(());
    };
    let Some(path) = reference
        .as_str()
        .and_then(|value| value.strip_prefix("$/"))
    else {
        return Ok(());
    };
    ensure!(
        immutable_reference(reference.as_str().context("reference must be a string")?),
        "invalid self-repository reference"
    );
    let projected = format!("./{path}");
    mapping.set("uses", projected.as_str());
    *reference = Yaml::String(projected);
    Ok(())
}

pub fn actionlint_source(source: &Source) -> Result<String> {
    let mut expected = parse(&source.text)?;
    let editor = yaml_edit::YamlFile::from_str(&source.text)
        .context("workflow syntax cannot be preserved")?;
    let document = editor
        .documents()
        .next()
        .context("workflow document is missing")?;
    let root = document
        .as_mapping()
        .context("workflow root must be a mapping")?;
    let editor_jobs = root.get_mapping("jobs").context("jobs must be a mapping")?;
    let Yaml::Hash(root_fields) = &mut expected else {
        anyhow::bail!("workflow root must be a mapping")
    };
    let Some(Yaml::Hash(jobs)) = root_fields.get_mut(&Yaml::String("jobs".to_owned())) else {
        anyhow::bail!("jobs must be a mapping")
    };
    for (id, job) in jobs {
        let id = id.as_str().context("job ID must be a string")?;
        let editor_job = editor_jobs
            .get_mapping(id)
            .context("job cannot be projected")?;
        project_reference(&editor_job, job)?;
        let Yaml::Hash(fields) = job else {
            anyhow::bail!("job must be a mapping")
        };
        if let Some(Yaml::Array(steps)) = fields.get_mut(&Yaml::String("steps".to_owned())) {
            let editor_steps = editor_job
                .get_sequence("steps")
                .context("steps cannot be projected")?;
            for (index, step) in steps.iter_mut().enumerate() {
                let editor_step = editor_steps
                    .get(index)
                    .and_then(|node| node.as_mapping().cloned())
                    .context("step cannot be projected")?;
                project_reference(&editor_step, step)?;
            }
        }
    }
    let projected = editor.to_string();
    ensure!(
        parse(&projected)? == expected,
        "linter projection changed workflow semantics outside self-reference spelling"
    );
    Ok(projected)
}

fn event_names(events: &Yaml) -> Result<Vec<String>> {
    if let Some(event) = events.as_str() {
        return Ok(vec![event.to_owned()]);
    }
    if let Some(events) = events.as_vec() {
        return events
            .iter()
            .map(|event| {
                event
                    .as_str()
                    .map(str::to_owned)
                    .context("event must be a string")
            })
            .collect();
    }
    if let Some(events) = events.as_hash() {
        return events
            .keys()
            .map(|event| {
                event
                    .as_str()
                    .map(str::to_owned)
                    .context("event key must be a string")
            })
            .collect();
    }
    anyhow::bail!("workflow must declare events");
}

fn missing(value: &Yaml) -> bool {
    matches!(value, Yaml::BadValue)
}

#[derive(Clone, Copy, PartialEq, Eq)]
#[cfg_attr(kani, derive(kani::Arbitrary))]
pub enum CheckoutAccess {
    ReadOnly,
    ContentsWriter,
}

pub fn credentials_permitted(access: CheckoutAccess, disabled: bool) -> bool {
    disabled || access == CheckoutAccess::ContentsWriter
}

fn checkout_access(permissions: &Yaml) -> CheckoutAccess {
    if permissions.as_str() == Some("write-all")
        || permissions["contents"].as_str() == Some("write")
    {
        CheckoutAccess::ContentsWriter
    } else {
        CheckoutAccess::ReadOnly
    }
}

fn only_noop_steps(job: &Yaml) -> bool {
    let Some(steps) = job["steps"].as_vec() else {
        return false;
    };
    !steps.is_empty()
        && steps.iter().all(|step| {
            step["run"].as_str().is_some_and(|run| {
                let run = run.trim();
                run == "true"
                    || (run.starts_with("echo ")
                        && !run.contains(['\n', ';', '|', '&', '$', '`', '<', '>']))
            })
        })
}

pub fn analyze(repository: &str, source: &Source) -> Result<WorkflowSummary> {
    let document = parse(&source.text).with_context(|| format!("{repository}/{}", source.name))?;
    let events = event_names(&document["on"])?;
    let jobs = document["jobs"]
        .as_hash()
        .context("jobs must be a mapping")?;
    ensure!(!jobs.is_empty(), "workflow has no jobs");
    let mut summary = WorkflowSummary {
        repository: repository.to_owned(),
        workflow: source.name.clone(),
        name: document["name"].as_str().unwrap_or(&source.name).to_owned(),
        events,
        jobs: Vec::new(),
        actions: Vec::new(),
        commands: Vec::new(),
        findings: Vec::new(),
    };
    let finding = |job: Option<&str>, rule, detail: &str| Finding {
        repository: repository.to_owned(),
        workflow: source.name.clone(),
        job: job.map(str::to_owned),
        rule,
        detail: detail.to_owned(),
    };
    if missing(&document["permissions"]) {
        summary.findings.push(finding(
            None,
            Rule::ExplicitPermissions,
            "workflow token permissions are implicit",
        ));
    }
    if document["permissions"].as_str() == Some("write-all") {
        summary.findings.push(finding(
            None,
            Rule::ExcessivePermissions,
            "workflow grants every write permission",
        ));
    }
    if summary.events.iter().any(|event| event == "pull_request")
        && missing(&document["concurrency"])
    {
        summary.findings.push(finding(
            None,
            Rule::PullRequestConcurrency,
            "superseded pull request checks are not coordinated",
        ));
    }
    for (id, job) in jobs {
        let id = id.as_str().context("job ID must be a string")?;
        ensure!(job.as_hash().is_some(), "job {id} must be a mapping");
        summary.jobs.push(id.to_owned());
        if job["permissions"].as_str() == Some("write-all") {
            summary.findings.push(finding(
                Some(id),
                Rule::ExcessivePermissions,
                "job grants every write permission",
            ));
        }
        if let Some(reference) = job["uses"].as_str() {
            summary.actions.push(reference.to_owned());
            if !immutable_reference(reference) {
                summary
                    .findings
                    .push(finding(Some(id), Rule::ImmutableAction, reference));
            }
            continue;
        }
        if job["timeout-minutes"]
            .as_i64()
            .is_none_or(|timeout| !(1..=360).contains(&timeout))
        {
            summary.findings.push(finding(
                Some(id),
                Rule::JobTimeout,
                "job needs an explicit positive timeout of at most 360 minutes",
            ));
        }
        let permissions = if missing(&job["permissions"]) {
            &document["permissions"]
        } else {
            &job["permissions"]
        };
        let steps = job["steps"]
            .as_vec()
            .context("normal job must contain steps")?;
        for step in steps {
            if let Some(reference) = step["uses"].as_str() {
                summary.actions.push(reference.to_owned());
                if !immutable_reference(reference) {
                    summary
                        .findings
                        .push(finding(Some(id), Rule::ImmutableAction, reference));
                }
                if reference.starts_with("actions/checkout@")
                    && !credentials_permitted(
                        checkout_access(permissions),
                        step["with"]["persist-credentials"].as_bool() == Some(false)
                            || step["with"]["persist-credentials"].as_str() == Some("false"),
                    )
                {
                    summary.findings.push(finding(
                        Some(id),
                        Rule::CheckoutCredentials,
                        "read-only checkout retains credentials",
                    ));
                }
            }
            if let Some(command) = step["run"].as_str() {
                summary.commands.push(command.to_owned());
            }
        }
        if (id == "required" || job["name"].as_str() == Some("required"))
            && missing(&job["needs"])
            && only_noop_steps(job)
        {
            summary.findings.push(finding(
                Some(id),
                Rule::NoOpRequired,
                "required check performs no verification",
            ));
        }
    }
    Ok(summary)
}

#[cfg(kani)]
mod proofs {
    use super::{CheckoutAccess, credentials_permitted};

    #[kani::proof]
    fn read_only_checkout_cannot_retain_credentials() {
        let access: CheckoutAccess = kani::any();
        let disabled: bool = kani::any();
        let accepted = credentials_permitted(access, disabled);
        assert_eq!(
            credentials_permitted(CheckoutAccess::ReadOnly, disabled),
            disabled
        );
        assert!(!accepted || disabled || access == CheckoutAccess::ContentsWriter);
        kani::cover!(accepted);
        kani::cover!(!accepted);
    }
}
