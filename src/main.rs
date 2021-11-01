#![feature(unboxed_closures)]
#![feature(fn_traits)]

///Program needs to return error and close down for program to run correctly(they should do that anyway)
extern crate git2;
extern crate mio_child_process;
extern crate walkdir;
extern crate yaml_rust;

use git2::build::RepoBuilder;
use git2::{Cred, FetchOptions, RemoteCallbacks};
use std::path::Path;

use std::process::Command;

use std::str;

use mio_child_process::CommandAsync;
use std::fs;
use std::fs::canonicalize;
use std::fs::File;
use std::io::prelude::*;
use std::sync::mpsc::TryRecvError;
use std::{thread, time};
use walkdir::WalkDir;
use yaml_rust::YamlLoader;

#[derive(Debug)]
struct Project<S>
where
    S: Fn(),
{
    name: String,
    repo_url: String,
    last_sha: String,
    new_sha: String,
    is_old: bool,
    path: String,
    port: String,
    start_up: S,
}

impl<S> Project<S>
where
    S: Fn() + Sync,
{
    fn new(
        name: String,
        repo_url: String,
        last_sha: String,
        new_sha: String,
        path: String,
        port: String,
        start_up: S,
    ) -> Project<S> {
        Project {
            name,
            repo_url,
            last_sha,
            new_sha,
            is_old: false,
            path,
            port,
            start_up,
        }
    }

    ///Check remote sha
    fn check_last_remote_sha(&mut self) {
        let repo_sha = Command::new("git")
            .arg("ls-remote")
            .arg(&self.repo_url)
            .output()
            .expect("failed to exectue command");

        let s = str::from_utf8(&repo_sha.stdout).unwrap().to_string();
        let result: Vec<&str> = s.split("\t").collect();
        let sha = result[0].to_string();
        if sha != self.last_sha {
            println!("sha mismatch in check_last_remote_sha");
            self.new_sha = sha.clone();
            self.is_old = true;
        }
    }

    fn count_files_in_folder(&self) -> usize {
        let total_size = WalkDir::new(self.path.clone())
            .min_depth(1)
            .max_depth(2)
            .into_iter()
            .filter_map(|entry| entry.ok())
            .filter_map(|entry| entry.metadata().ok())
            .filter(|metadata| metadata.is_file())
            .count();

        total_size
    }

    ///check if project need redeployment return true if needs it
    fn check_if_old(&self) -> bool {
        self.is_old.clone()
    }

    fn kill_process_remove_old_files(&self) {
        self.kill_running_process();
        self.remove_old_files_from_disk();
    }

    fn startup_check_if_need_redownload(&self) -> bool {
        if self.count_files_in_folder() > 3 {
            return false;
        } else {
            return true;
        }
    }

    fn clean_old_and_redownload_repo(&self) {
        println!("In function clean_old_and_redownload_repo");
        self.kill_process_remove_old_files();
        self.check_if_files_got_deleted();
        self.clone_git_repo();
    }
    
    fn redeployed(&mut self) -> bool {
        if self.check_if_old() {
            self.kill_process_remove_old_files();
            self.last_sha = self.new_sha.clone();
            self.is_old = false;
            thread::sleep(time::Duration::from_millis(800)); //Should fix message of missing file on clonning
            self.check_if_files_got_deleted();
            self.clone_git_repo();

            (self.start_up)();
        }
        true
    }

    fn check_if_files_got_deleted(&self) {
        loop {
            if !Path::new(&self.path).exists() {
                break;
            }
            //Hold in loop if file still exists
            thread::sleep(time::Duration::from_secs(1));
        }
    }

    fn startup_project(&mut self) {
        println!("in function startup_project");
        self.kill_running_process();
        self.redeployed();
        (self.start_up)();
    }

    ///Clones remote repo to destination
    fn clone_git_repo(&self) {
        let repo_url = self.repo_url.clone();
        let repo_clone_path = self.path.clone();

        println!("Cloning {} into {}", repo_url, repo_clone_path);

        let mut builder = RepoBuilder::new();
        let mut callbacks = RemoteCallbacks::new();
        let mut fetch_options = FetchOptions::new();

        callbacks.credentials(|_, _, _| {
            let credentials = Cred::ssh_key(
                "git",
                Some(Path::new("xxxx/id_rsa.pub")), // Path to  publickey
                Path::new("xxxx/id_rsa"), // Path to privatekey
                Some("xxxx"), // Password to keys
            )
            .expect("Could not create credentials object");
            Ok(credentials)
        });

        fetch_options.remote_callbacks(callbacks);

        builder.fetch_options(fetch_options);
        builder
            .clone(&repo_url, Path::new(&repo_clone_path))
            .expect("Could not clone repo");

        println!("Clone complete");
    }

    fn kill_running_process(&self) {
        let netstat_result = Command::new("fuser")
            .arg("-v")
            .arg("-n")
            .arg("tcp")
            .arg(self.port.clone())
            .output()
            .expect("Can't process with command.");

        let pid = String::from_utf8(netstat_result.stdout).unwrap();
        Command::new("kill")
            .arg(pid)
            .spawn()
            .expect("Couldn't kill process");
    }

    fn remove_old_files_from_disk(&self) {
        Command::new("rm")
            .arg("-rf")
            .arg(self.path.clone())
            .spawn()
            .expect("Cant remove project folder");
    }
    ///Function to change sha from old to new one.
    fn save_new_sha_to_file(&self) {
        let mut src = File::open("config.yaml").expect("Could open file to save.");
        let mut data = String::new();
        src.read_to_string(&mut data)
            .expect("Couldn't read from file to string.");
        drop(src);
        println!("{:?}", &self.new_sha);
        if self.new_sha == "" {
            println!("Empty string returning.");
            return;
        }
        let new_data = data.replace(&self.last_sha, &self.new_sha);
        let mut dst = File::create("config.yaml").expect("Couldn't create file to save.");
        dst.write(new_data.as_bytes())
            .expect("Couldn't save file to disk.");
    }
}

fn main() {
    let mut f = File::open("config.yaml").expect("File not found");
    let mut contents = String::new();
    f.read_to_string(&mut contents).unwrap();
    let docs = YamlLoader::load_from_str(&contents).expect("Cant load from file string!");
    let doc = &docs[0];
    let mut projects_vector = Vec::new();

    let mut index = 0;
    while doc[index]["name"] != yaml_rust::Yaml::BadValue {
        let project = Project::new(
            doc[index]["name"].clone().into_string().unwrap(),
            doc[index]["repo"].clone().into_string().unwrap(),
            doc[index]["lastSHA"].clone().into_string().unwrap(),
            String::from(""),
            doc[index]["path"].clone().into_string().unwrap(),
            format!("{:?}", doc[index]["port"].clone())
                .replace("Integer(", "")
                .replace(")", ""),
            move || {
                let mut i = 0;
                while doc[index]["run"][i] != yaml_rust::Yaml::BadValue {
 //                   println!("In function startup");
                    let mut change_cwd: Vec<String> = Vec::new();
                    let mut extracted_command = format!("{:?}", doc[index]["run"][i]);
 //                   println!("extracted_command: {}", &extracted_command);
                    extracted_command = extracted_command
                        .replace("String(\"", "")
                        .replace("\")", "");

                    let command_to_exec = extracted_command.split(" ").map(|s| s.to_owned());

                    if extracted_command.starts_with("cd ") {
                        println!("Command starts with cd");
                        for subcommand in command_to_exec.clone() {
                            change_cwd.push(subcommand);
                        }
                    }
                    let mut current_directory;
                    if change_cwd.len() > 0 {
                        current_directory = doc[index]["path"].clone().into_string().unwrap()
                            + "/"
                            + &change_cwd[1]; // have to add / becase ./ dot is treat like folder name
                    } else {
                        current_directory = doc[index]["path"].clone().into_string().unwrap();
                    }
                    current_directory = current_directory.replace("./", "");

                    if !Path::new(&current_directory).exists() {
   //                     println!("Folder not existing creating");
   //                     format!("Created directory at {} ", current_directory.to_string());
                        fs::create_dir_all(&current_directory).expect(&format!(
                            "Couldn't make directory at {}",
                            &current_directory
                        ));
                    } else {
   //                     println!("Folder existing skiping creation.");
                    }

                    let mut command_vector: Vec<String> = Vec::new();

                    for subcommand in command_to_exec.clone() {
                        command_vector.push(subcommand);
                    }
                    if change_cwd.len() > 0 {
                        // we need to take next command to process
    //                    println!("i in change_cwd.len() {}", i);
                        let mut extracted_command = format!("{:?}", doc[index]["run"][i + 1]);
                        extracted_command = extracted_command
                            .replace("String(\"", "")
                            .replace("\")", "");
                        let command_to_exec = extracted_command.split(" ");

                        let mut command_vector: Vec<String> = Vec::new();

                        for subcommand in command_to_exec.clone() {
                            command_vector.push(subcommand.to_string());
                        }
                        //process next command
                        let mut command_to_exec = Command::new(&command_vector[0]);

                        //                        println!("current_directory after change {:?}", current_directory);
                        command_to_exec.current_dir(
                            canonicalize(&current_directory)
                                .expect("Can't parse change directory path"),
                        );

                        for item in 1..command_vector.len() {
                            command_to_exec.arg(&command_vector[item]);
                        }
                        command_to_exec.output().expect(&format!(
                            "Startup command faild for {} project",
                            doc[index]["name"].clone().into_string().unwrap()
                        ));
   //                     println!("Command executed {:?} after cd!!!", command_vector[0]);
                        i += 2; //add 2 becase we took +1 to process higher
                    } else {
                        let mut download_modules = Command::new(&command_vector[0]);
                        println!(
                            "Executing command without folder change {}",
                            command_vector[0]
                        );
                        download_modules
                            .current_dir(doc[index]["path"].clone().into_string().unwrap());
                        for item in 1..command_vector.len() {
                            println!("Command to pass {}", command_vector[item]);
                            download_modules.arg(&command_vector[item]);
                        }
                        download_modules.spawn().expect(&format!(
                            "Startup command faild for {} project",
                            doc[index]["name"].clone().into_string().unwrap()
                        )).wait().unwrap();
                        i += 1;
                    }
                }
                //CMD to run program
                let mut extracted_command = format!("{:?}", doc[index]["cmd"]);
                extracted_command = extracted_command
                    .replace("String(\"", "")
                    .replace("\")", "");

                let mut change_dir_vector: Vec<&str> = Vec::new();
                //TODO remove cd and folder command from string and add folder to current directory
                if extracted_command.starts_with("cd ") {
                    change_dir_vector = extracted_command.split("&&").collect();
                }

                let mut command_to_exec = extracted_command.split(" ");
                if change_dir_vector.len() > 0 {
                    command_to_exec = change_dir_vector[1].split(" ");
                }

                let mut command_vector: Vec<&str> = Vec::new();

                for subcommand in command_to_exec.clone() {
                    command_vector.push(subcommand);
                }
                for item in command_to_exec.clone() {
                    println!("{}", item);
                }

                // let mut download_modules = Command::new(&command_vector[1]);
                let mut download_modules;
                if change_dir_vector.len() > 0 {
                    download_modules = Command::new(&command_vector[1]);
                } else {
                    download_modules = Command::new(&command_vector[0]);
                }

                let current_directory;
                if change_dir_vector.len() > 0 {
                    current_directory = doc[index]["path"].clone().into_string().unwrap()
                        + "/"
                        + &change_dir_vector[0].replace("cd ", "").trim();
                    println!("current directory  if len > 0 {}", &current_directory);
                } else {
                    current_directory = doc[index]["path"].clone().into_string().unwrap();
                    println!("current directory  if len < 0 {}", &current_directory);
                }
                download_modules.current_dir(&current_directory);

                if change_dir_vector.len() > 0 {
                    for item in 2..command_vector.len() {
                        println!("adding argument: {}", command_vector[item]);
                        download_modules.arg(command_vector[item].trim());
                    }
                } else {
                    for item in 1..command_vector.len() {
                        println!("adding argument: {}", command_vector[item]);
                        download_modules.arg(command_vector[item].trim());
                    }
                }

                let mut status = download_modules
                    .spawn_async()
                    .expect("Can't execute command.");

                let mut trys_without_error = 0;
                loop {
                    thread::sleep(time::Duration::from_secs(20));
                    match status.try_recv() {
                        Ok(r) => r,
                        Err(TryRecvError::Empty) => {
                            println!("In Empty Arm {}", trys_without_error);
                            trys_without_error += 1;
                            println!("In Empty Arm {}", trys_without_error);
                            if trys_without_error > 3 {
                                break;
                            };
                            continue;
                        }
                        Err(TryRecvError::Disconnected) => {
                            println!("Program crashed restarting");
                            //status.kill().expect("Couldn't kill running process");

                            download_modules
                                .spawn_async()
                                .expect("Can't execute command.");
                            break;
                        }
                    };

                    //                println!("{:?}", result);
                }
            },
        );
        projects_vector.push(project);
        index += 1;
    }

    projects_vector[0].check_last_remote_sha();
    println!(
        "Is This project old ?? {:?}",
        projects_vector[0].check_if_old()
    );
    projects_vector[0].save_new_sha_to_file();

    for project in &mut projects_vector {
        println!(
            "Do project {} need redownload {}",
            project.name,
            project.startup_check_if_need_redownload()
        );

        if project.startup_check_if_need_redownload() {
            project.clean_old_and_redownload_repo();
        }
        project.startup_project();
    }

    loop {
        for project in &mut projects_vector {
            project.check_last_remote_sha();
            if project.check_if_old() {
                project.kill_process_remove_old_files();
                project.save_new_sha_to_file();
                project.redeployed();
            }
        }
        println!("In outer loop of projects.");
        thread::sleep(time::Duration::from_secs(60));
    }
}
