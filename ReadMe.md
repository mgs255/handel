# Handel - 'composer' of docker-compose files 

![Build status](https://github.com/mgs255/handel/actions/workflows/build.yml/badge.svg?branch=main)

This is simple command line application that takes away a lot of
the pain when interacting with the `docker` and `docker-compose` tools in 
development environments.  The tool is heavily based on/inspired by the container-juggler tool
available from [github](https://github.com/sgeisbacher/container-juggler).

The tool assists the developer in two specific ways:

  * Automating the construction of a `docker-compose.yml`.  The tool does 
    this by constructing a set of Dockerfile 'fragments' based on selecting
    one of a specified set of 'scenarios'.
    
  * Synchronises the versions of the images used in the docker-compose files 
    using a combination of the local docker registry and the versions specified 
    via an external reference system, which is assumed to be accessible via an 
    HTTP GET request. 
        
Unlike the container-juggler tool, scenarios nest and also use the docker-compose
'depends_on' directive to construct a list of scenario dependenies.
        
## Usage
 
```
USAGE:
    handel [FLAGS] [OPTIONS] [scenario]

FLAGS:
    -h, --help       Prints help information
    -V, --version    Prints version information
    -v               Sets the level of verbosity

OPTIONS:
    -c <config>            Sets the configuration file to use [default: handel.yml]
    -e, --env <env>        The environment from which container versions are copied. [default: test]  [possible values:
                           dev, test, staging, prod]
    -s, --since <since>    Maxiumum age of locally built containers to consider.  This uses time units s,m,h,d,w which
                           should be prefixed by an integer, e.g. 15m, 3h, 2d etc. [default: 1d]

ARGS:
    <scenario>    Sets the scenario to use
```

In a nutshell to use it to create a compose-file based on the profile `cq`
for the 'content-query' service plus the service in the profile plus any dependencies
it requires, we can use something like:

`handle --since=1d --env=test cq`

The additional arguments here will override the versions of any local 
image older than 1 day and instead use the version which matches the 
service version running in the test reference environment.

If the compose file fragments have local volumes specified, those volumes
can be initialised by extracting a zip file which is retrieved some a location 
on the local machine, or pulled from an S3 bucket.  See the example below for both 
configurations.  If the given directory is empty, then the volume initialisation step
will be skipped.

**Note** that these volumes must also be mapped to the docker host, and that the S3 
bucket (if used) must be accessible from the current environment.  In practice this means 
that default configuration for the AWS CLI must allow access to any given S3 URIs.

## AWS Credentials Note

The S3 integration is provided by a currently alpha version of the AWS Rust SDK.
At this time, this SDK only supports providing credentials via environment
variables, rather than using the standard credential chain behaviour of other
AWS tools.  

In order to use handel to download binaries from S3 you **must** export
your default AWS credential KEY and SECRET to the current environment. In addition, 
if the S3 bucket you are referencing is not located in us-east-1, then the `AWS_REGION`
 environment variable will need to used to define that.  The examples in this file 
are publicly readable accessible S3 objects in the eu-west-2 region.

An example of setting the environment: 

```bash
$ export AWS_ACCESS_KEY_ID=$(aws configure get default.aws_access_key_id)
$ export AWS_SECRET_ACCESS_KEY=$(aws configure get default.aws_secret_access_key)
$ export AWS_REGION=eu-west-2
```

## Configuration file format 
  
The configuration file is yaml, and has 4 sections:

* template-folder-path (string): path containing the docker-compose fragments.
* reference (object - optional): an HTTP endpoint from which to fetch a list of 
  'reference' versions of a service.  If there is no local image which is more
  recent than the 'since' time, then that reference version will be used instead. 
* scenarios (map):  a map of scenario names to services.  Each entry can be 
either the name of a fragment file (in the template directory without the yml extension) 
or another scenario.
* volume-init: List of name, source, target objects.  The source can be either a path 
  to a file on the local filesystem or it can be a S3 URI.  Environment variables will
  be expanded if found.

As an example: 

```yaml
template-folder-path: ./data/templates/

reference:
  url: https://mgs-example-s3.s3.eu-west-2.amazonaws.com/versions-{env}.json
  env-mappings:
    prod: production
  jq-filter: '. | [to_entries[] | { "name": .key, "version": ( .value | to_entries[0].key ) }] '

scenarios:
  db:
    - mysql
  
  core:
    - zookeeper
    - kafka
    - redis
    - memcached
 
  app:
    - db
    - core

  cq:
    - app
    - content-query

volumes:
  - name: local-volume-example
    source: $PWD/data/volume-src/example-a.zip
    target:  $PWD/data/volumes/example-a
  - name: s3-volume-example
    source: s3://mgs-example-s3/volume/example-b.zip
    target:  $PWD/data/volumes/example-b
  - name: mysql-data
    source: s3://mgs-example-s3/volume/volume/mysql-base.zip
    target:  $PWD/data/volumes/mysql
```
  
## The fragment file format 

*Note:*  In order to be able to compare versions, the fragment-file, image-name and service
  name in the reference system are all expected to match.
  
Each fragment must have at minimum at least an image entry:

* image: [the docker image uri](https://docs.docker.com/compose/compose-file/compose-file-v3/#image) 
* depends_on: [a list of services that this service requires in order to run](https://docs.docker.com/compose/compose-file/compose-file-v3/#depends_on)
* restart: [one of "no","always","unless-stopped" or "on-failure"](https://docs.docker.com/compose/compose-file/compose-file-v3/#restart)
* environment: [a map of environment variables to provide to the container](https://docs.docker.com/compose/compose-file/compose-file-v3/#environment)
* ports: [list of source:target pairs to define port mappings](https://docs.docker.com/compose/compose-file/compose-file-v3/#ports) 
* ports: [list of source:target pairs to define port mappings](https://docs.docker.com/compose/compose-file/compose-file-v3/#ports)

For example, we might define a service content-query service file (named content-query.yml)
 having the following: 

```yaml
image: 121212121.dkr.ecr.us-east-1.amazonaws.com/content-query:1.0.4
restart: always
depends_on:
    - mysql
    - kafka
environment:
    SPRING_KAFKA_BOOTSTRAPSERVERS: kafka:9092
    JAVA_TOOL_OPTIONS: "-agentlib:jdwp=transport=dt_socket,server=y,suspend=n,address=*:5005"
    DATABASE_PASSWORD: ""
ports:
    - 9149:8080
    - 7932:5005
```

The program will ignore the respository prefix, and extract the service name (again assuming 
that the service name has the same name as the image).  Note that in this case, the service 
depends on mysql and kafka and these services will be added as required dependent services.

## Reference system

The reference system can be set up using an HTTP source, which defines a list of versions to use as JSON.
There are a 3 aspects of this which can be configured:

* `url` - the HTTP endpoint from which the versions can be retrieved, currently this must be an 
open HTTP endpoint, this is assumed to return a JSON object or array.
* `env_mappings` - allows defining a set of mappings between dev, test, prod & staging and any other 
names you like.  The url's {env} is replaced with the value of the env mapping.  This section may be 
ommitted.
* `jq_filter` - a jq script to convert the JSON body, into a JSON array.  If this field is defined 
  the program will attempt to spawn the jq tool piping in the JSON body from the given URL, 
  and read its output.  The output of the filtered JSON body is expected to be 
  a JSON array of the form: 

```json
[ 
  { "name": "service_1_name", "version": "service_1_version" },
  { "name": "service_2_name", "version": "service_2_version" },
  ...
  { "name": "service_n_name", "version": "service_n_version" },
]

```

## Building

Requires a recent version of rust.

```
$ rustup  show
   ...
   stable-x86_64-apple-darwin (default)
   rustc 1.53.0 (53cb7b09b 2021-06-17)
```

### Building the binary 

`cargo build --release`

### Running the binary

The binary will be built to:

`target/release/handel`  

This should be copied to somewhere on the PATH.

## AWS SDK Logging

`RUST_LOG='smithy_http_tower::dispatch=trace,smithy_http::middleware=trace'` handel ..
 
## To do

* ~~Lots of cleaning up~~
* ~~Downloading dependencies via S3~~
* ~~Better error handling/messages/stack-traces.~~
* ~~Expand env vars for volume initialisation~~ 
* ~~Initialisation of data directories/volumes based on zip files~~
* ~~Fix windows build/execution issues~~ 
* ~~Make 'reference' system more generic - i.e: allow the configuration of the URL + means of extracting service 
  names/versions.~~
* Automate detection of port conflicts
* Suggestion of ports to free ports 

  
