use super::filter::IntoFilter;
use super::query_arguments::into_model_query_arguments;
use crate::{
    protobuf::{prelude::*, InputValidation},
    BridgeError, BridgeResult, ExternalInterface,
};
use connector::*;
use prisma_common::{config::WithMigrations, config::*};
use prisma_models::prelude::*;
use prost::Message;
use sqlite_connector::{SqlResolver, Sqlite};
use std::sync::Arc;

pub struct ProtoBufInterface {
    data_resolver: Box<dyn DataResolver + Send + Sync + 'static>,
    database_mutaction_executor: Arc<DatabaseMutactionExecutor + Send + Sync + 'static>,
}

impl ProtoBufInterface {
    pub fn new(config: &PrismaConfig) -> ProtoBufInterface {
        let (data_resolver, mutaction_executor) = match config.databases.get("default") {
            Some(PrismaDatabase::Explicit(ref config))
                if config.connector == "sqlite-native" || config.connector == "native-integration-tests" =>
            {
                // FIXME: figure out the right way to do it
                // we are passing is_active as test_mode parameter
                // this requires us to put `active: true` in all sqlite-native configs used in tests
                let sqlite = Arc::new(Sqlite::new(config.limit(), config.is_active().unwrap()).unwrap());

                (SqlResolver::new(sqlite.clone()), sqlite)
            }
            _ => panic!("Database connector is not supported, use sqlite with a file for now!"),
        };

        ProtoBufInterface {
            data_resolver: Box::new(data_resolver),
            database_mutaction_executor: mutaction_executor,
        }
    }

    fn protobuf_result<F>(f: F) -> Vec<u8>
    where
        F: FnOnce() -> BridgeResult<Vec<u8>>,
    {
        f().unwrap_or_else(|error| match error {
            BridgeError::ConnectorError(ConnectorError::NodeDoesNotExist) => {
                let response = prisma::RpcResponse::empty();
                let mut response_payload = Vec::new();

                response.encode(&mut response_payload).unwrap();
                response_payload
            }
            _ => {
                dbg!(&error);

                let error_response = prisma::RpcResponse::error(error);

                let mut payload = Vec::new();
                error_response.encode(&mut payload).unwrap();
                payload
            }
        })
    }
}

impl InputValidation for GetNodeByWhereInput {
    fn validate(&self) -> BridgeResult<()> {
        Ok(())
    }
}

impl ExternalInterface for ProtoBufInterface {
    fn get_node_by_where(&self, payload: &mut [u8]) -> Vec<u8> {
        Self::protobuf_result(|| {
            let input = GetNodeByWhereInput::decode(payload)?;
            input.validate()?;

            let project_template: ProjectTemplate = serde_json::from_reader(input.project_json.as_slice())?;
            let project: ProjectRef = project_template.into();

            let model = project.schema().find_model(&input.model_name)?;
            let selected_fields = input.selected_fields.into_selected_fields(model.clone(), None);

            let value: PrismaValue = input.value.into();
            let field = model.fields().find_from_scalar(&input.field_name)?;
            let node_selector = NodeSelector { field, value };

            let query_result = self.data_resolver.get_node_by_where(node_selector, &selected_fields)?;

            let (nodes, fields) = match query_result {
                Some(node) => (vec![node.node.into()], node.field_names),
                _ => (Vec::new(), Vec::new()),
            };

            let response = RpcResponse::ok(prisma::NodesResult { nodes, fields });

            let mut response_payload = Vec::new();
            response.encode(&mut response_payload).unwrap();

            Ok(response_payload)
        })
    }

    fn get_nodes(&self, payload: &mut [u8]) -> Vec<u8> {
        Self::protobuf_result(|| {
            let input = GetNodesInput::decode(payload)?;
            input.validate()?;

            let project_template: ProjectTemplate = serde_json::from_reader(input.project_json.as_slice())?;
            let project: ProjectRef = project_template.into();

            let model = project.schema().find_model(&input.model_name)?;
            let selected_fields = input.selected_fields.into_selected_fields(model.clone(), None);
            let query_arguments = into_model_query_arguments(model.clone(), input.query_arguments);

            let query_result = self.data_resolver.get_nodes(model, query_arguments, &selected_fields)?;
            let (nodes, fields) = (query_result.nodes, query_result.field_names);
            let proto_nodes = nodes.into_iter().map(|n| n.into()).collect();

            let response = RpcResponse::ok(prisma::NodesResult {
                nodes: proto_nodes,
                fields: fields,
            });

            let mut response_payload = Vec::new();
            response.encode(&mut response_payload).unwrap();

            Ok(response_payload)
        })
    }

    fn get_related_nodes(&self, payload: &mut [u8]) -> Vec<u8> {
        Self::protobuf_result(|| {
            let input = GetRelatedNodesInput::decode(payload)?;
            input.validate()?;

            let project_template: ProjectTemplate = serde_json::from_reader(input.project_json.as_slice())?;

            let project: ProjectRef = project_template.into();
            let model = project.schema().find_model(&input.model_name)?;

            let from_field = model.fields().find_from_relation_fields(&input.from_field)?;
            let from_node_ids: Vec<GraphqlId> = input.from_node_ids.into_iter().map(GraphqlId::from).collect();
            let related_model = from_field.related_model();

            let selected_fields = input
                .selected_fields
                .into_selected_fields(Arc::clone(&related_model), Some(from_field.clone()));

            let query_result = self.data_resolver.get_related_nodes(
                from_field,
                &from_node_ids,
                into_model_query_arguments(Arc::clone(&related_model), input.query_arguments),
                &selected_fields,
            )?;

            let (nodes, fields) = (query_result.nodes, query_result.field_names);
            let proto_nodes = nodes.into_iter().map(|n| n.into()).collect();

            let response = RpcResponse::ok(prisma::NodesResult {
                nodes: proto_nodes,
                fields: fields,
            });

            let mut response_payload = Vec::new();
            response.encode(&mut response_payload).unwrap();

            Ok(response_payload)
        })
    }

    fn get_scalar_list_values_by_node_ids(&self, payload: &mut [u8]) -> Vec<u8> {
        Self::protobuf_result(|| {
            let input = GetScalarListValuesByNodeIds::decode(payload)?;
            input.validate()?;

            let project_template: ProjectTemplate = serde_json::from_reader(input.project_json.as_slice())?;
            let project: ProjectRef = project_template.into();

            let model = project.schema().find_model(&input.model_name)?;
            let list_field = model.fields().find_from_scalar(&input.list_field)?;

            let node_ids: Vec<GraphqlId> = input.node_ids.into_iter().map(GraphqlId::from).collect();

            let query_result = self
                .data_resolver
                .get_scalar_list_values_by_node_ids(list_field, node_ids)?;

            let proto_values = query_result
                .into_iter()
                .map(|vals| prisma::ScalarListValues {
                    node_id: vals.node_id.into(),
                    values: vals.values.into_iter().map(|n| n.into()).collect(),
                })
                .collect();

            let response = RpcResponse::ok(prisma::ScalarListValuesResult { values: proto_values });

            let mut response_payload = Vec::new();
            response.encode(&mut response_payload).unwrap();

            Ok(response_payload)
        })
    }

    fn count_by_model(&self, payload: &mut [u8]) -> Vec<u8> {
        Self::protobuf_result(|| {
            let input = CountByModelInput::decode(payload)?;
            input.validate()?;

            let project_template: ProjectTemplate = serde_json::from_reader(input.project_json.as_slice())?;
            let project: ProjectRef = project_template.into();
            let model = project.schema().find_model(&input.model_name)?;

            let query_arguments = into_model_query_arguments(model.clone(), input.query_arguments);
            let count = self.data_resolver.count_by_model(model, query_arguments)?;

            let response = RpcResponse::ok(count);

            let mut response_payload = Vec::new();
            response.encode(&mut response_payload).unwrap();

            Ok(response_payload)
        })
    }

    fn count_by_table(&self, payload: &mut [u8]) -> Vec<u8> {
        Self::protobuf_result(|| {
            let input = CountByTableInput::decode(payload)?;
            input.validate()?;

            let project_template: ProjectTemplate = serde_json::from_reader(input.project_json.as_slice())?;
            let project: ProjectRef = project_template.into();

            let count = match project.schema().find_model(&input.model_name) {
                Ok(model) => self
                    .data_resolver
                    .count_by_table(project.schema().db_name.as_ref(), model.db_name()),
                Err(_) => self
                    .data_resolver
                    .count_by_table(project.schema().db_name.as_ref(), &input.model_name),
            }?;

            let response = RpcResponse::ok(count);

            let mut response_payload = Vec::new();
            response.encode(&mut response_payload).unwrap();

            Ok(response_payload)
        })
    }

    fn execute_raw(&self, payload: &mut [u8]) -> Vec<u8> {
        Self::protobuf_result(|| {
            let input = ExecuteRawInput::decode(payload)?;
            let json = self.database_mutaction_executor.execute_raw(input.query)?;
            let json_as_string = serde_json::to_string(&json)?;

            let response = RpcResponse::ok_raw(prisma::ExecuteRawResult { json: json_as_string });
            let mut response_payload = Vec::new();

            response.encode(&mut response_payload).unwrap();

            Ok(response_payload)
        })
    }

    fn execute_mutaction(&self, payload: &mut [u8]) -> Vec<u8> {
        Self::protobuf_result(|| {
            let input = crate::protobuf::prisma::DatabaseMutaction::decode(payload)?;
            let project_template: ProjectTemplate = serde_json::from_reader(input.project_json.as_slice())?;
            let project: ProjectRef = project_template.into();
            let mutaction = convert_mutaction(input, Arc::clone(&project));
            let db_name = project.schema().db_name.to_string();

            let mut results = self.database_mutaction_executor.execute(db_name, mutaction)?;
            let result = results.pop().expect("no mutaction results returned");

            let response = RpcResponse::ok_mutaction(convert_mutaction_result(result));
            let mut response_payload = Vec::new();

            response.encode(&mut response_payload).unwrap();
            Ok(response_payload)
        })
    }
}

impl From<BridgeError> for super::prisma::error::Value {
    fn from(error: BridgeError) -> super::prisma::error::Value {
        match error {
            BridgeError::ConnectorError(e @ ConnectorError::ConnectionError(_)) => {
                super::prisma::error::Value::ConnectionError(format!("{}", e))
            }

            BridgeError::ConnectorError(e @ ConnectorError::QueryError(_)) => {
                super::prisma::error::Value::QueryError(format!("{}", e))
            }

            BridgeError::ConnectorError(e @ ConnectorError::InvalidConnectionArguments) => {
                super::prisma::error::Value::QueryError(format!("{}", e))
            }

            BridgeError::ConnectorError(ConnectorError::FieldCannotBeNull { field }) => {
                super::prisma::error::Value::FieldCannotBeNull(field)
            }

            BridgeError::ConnectorError(ConnectorError::UniqueConstraintViolation { field_name }) => {
                super::prisma::error::Value::UniqueConstraintViolation(field_name)
            }

            BridgeError::ConnectorError(ConnectorError::NodeNotFoundForWhere { model, field, value }) => {
                let node_selector = super::prisma::NodeSelector {
                    model_name: model,
                    field_name: field,
                    value: value.into(),
                };

                super::prisma::error::Value::NodeNotFoundForWhere(node_selector)
            }

            e @ BridgeError::ProtobufDecodeError(_) => {
                super::prisma::error::Value::ProtobufDecodeError(format!("{}", e))
            }

            e @ BridgeError::JsonDecodeError(_) => super::prisma::error::Value::JsonDecodeError(format!("{}", e)),

            e @ BridgeError::DomainError(_) => super::prisma::error::Value::InvalidInputError(format!("{}", e)),

            e => super::prisma::error::Value::InvalidInputError(format!("{}", e)),
        }
    }
}

fn convert_mutaction(m: crate::protobuf::prisma::DatabaseMutaction, project: ProjectRef) -> DatabaseMutaction {
    use crate::protobuf::prisma::database_mutaction;
    match m.type_.unwrap() {
        database_mutaction::Type::Create(x) => DatabaseMutaction::TopLevel(convert_create_envelope(x, project)),
        database_mutaction::Type::Update(x) => DatabaseMutaction::TopLevel(convert_update_envelope(x, project)),
        database_mutaction::Type::Upsert(x) => DatabaseMutaction::TopLevel(convert_upsert(x, project)),
        database_mutaction::Type::Delete(x) => DatabaseMutaction::TopLevel(convert_delete(x, project)),
        database_mutaction::Type::Reset(x) => DatabaseMutaction::TopLevel(convert_reset(x, project)),
        database_mutaction::Type::DeleteNodes(x) => DatabaseMutaction::TopLevel(convert_delete_nodes(x, project)),
        database_mutaction::Type::UpdateNodes(x) => DatabaseMutaction::TopLevel(convert_update_nodes(x, project)),
        database_mutaction::Type::NestedConnect(x) => {
            DatabaseMutaction::Nested(convert_nested_connect_envelope(x, project))
        }
        database_mutaction::Type::NestedDisconnect(x) => {
            DatabaseMutaction::Nested(convert_nested_disconnect_envelope(x, project))
        }
        database_mutaction::Type::NestedSet(x) => DatabaseMutaction::Nested(convert_nested_set_envelope(x, project)),
        database_mutaction::Type::NestedCreate(x) => {
            DatabaseMutaction::Nested(convert_nested_create_envelope(x, project))
        }
        database_mutaction::Type::NestedUpdate(x) => {
            DatabaseMutaction::Nested(convert_nested_update_envelope(x, project))
        }
        database_mutaction::Type::NestedUpsert(x) => {
            DatabaseMutaction::Nested(convert_nested_upsert_envelope(x, project))
        }
        database_mutaction::Type::NestedDelete(x) => {
            DatabaseMutaction::Nested(convert_nested_delete_envelope(x, project))
        }
        database_mutaction::Type::NestedUpdateNodes(x) => {
            DatabaseMutaction::Nested(convert_nested_update_nodes_envelope(x, project))
        }
        database_mutaction::Type::NestedDeleteNodes(x) => {
            DatabaseMutaction::Nested(convert_nested_delete_nodes_envelope(x, project))
        }
    }
}

fn convert_create_envelope(m: crate::protobuf::prisma::CreateNode, project: ProjectRef) -> TopLevelDatabaseMutaction {
    TopLevelDatabaseMutaction::CreateNode(convert_create(m, project))
}

fn convert_create(m: crate::protobuf::prisma::CreateNode, project: ProjectRef) -> CreateNode {
    let model = project.schema().find_model(&m.model_name).unwrap();
    CreateNode {
        model: model,
        non_list_args: convert_prisma_args(m.non_list_args),
        list_args: convert_list_args(m.list_args),
        nested_mutactions: convert_nested_mutactions(m.nested, Arc::clone(&project)),
    }
}

fn convert_nested_mutactions(m: crate::protobuf::prisma::NestedMutactions, project: ProjectRef) -> NestedMutactions {
    NestedMutactions {
        creates: m
            .creates
            .into_iter()
            .map(|m| convert_nested_create(m, Arc::clone(&project)))
            .collect(),
        updates: m
            .updates
            .into_iter()
            .map(|m| convert_nested_update(m, Arc::clone(&project)))
            .collect(),
        upserts: m
            .upserts
            .into_iter()
            .map(|m| convert_nested_upsert(m, Arc::clone(&project)))
            .collect(),
        deletes: m
            .deletes
            .into_iter()
            .map(|m| convert_nested_delete(m, Arc::clone(&project)))
            .collect(),
        connects: m
            .connects
            .into_iter()
            .map(|m| convert_nested_connect(m, Arc::clone(&project)))
            .collect(),
        disconnects: m
            .disconnects
            .into_iter()
            .map(|m| convert_nested_disconnect(m, Arc::clone(&project)))
            .collect(),
        sets: m
            .sets
            .into_iter()
            .map(|m| convert_nested_set(m, Arc::clone(&project)))
            .collect(),
        update_manys: m
            .update_manys
            .into_iter()
            .map(|m| convert_nested_update_nodes(m, Arc::clone(&project)))
            .collect(),
        delete_manys: m
            .delete_manys
            .into_iter()
            .map(|m| convert_nested_delete_nodes(m, Arc::clone(&project)))
            .collect(),
    }
}

fn convert_nested_create_envelope(
    m: crate::protobuf::prisma::NestedCreateNode,
    project: ProjectRef,
) -> NestedDatabaseMutaction {
    NestedDatabaseMutaction::CreateNode(convert_nested_create(m, project))
}

fn convert_nested_create(m: crate::protobuf::prisma::NestedCreateNode, project: ProjectRef) -> NestedCreateNode {
    let relation_field = find_relation_field(Arc::clone(&project), m.model_name, m.field_name);
    NestedCreateNode {
        relation_field: relation_field,
        non_list_args: convert_prisma_args(m.non_list_args),
        list_args: convert_list_args(m.list_args),
        top_is_create: m.top_is_create,
        nested_mutactions: convert_nested_mutactions(m.nested, Arc::clone(&project)),
    }
}

fn convert_update_envelope(m: crate::protobuf::prisma::UpdateNode, project: ProjectRef) -> TopLevelDatabaseMutaction {
    TopLevelDatabaseMutaction::UpdateNode(convert_update(m, project))
}

fn convert_update(m: crate::protobuf::prisma::UpdateNode, project: ProjectRef) -> UpdateNode {
    UpdateNode {
        where_: convert_node_select(m.where_, Arc::clone(&project)),
        non_list_args: convert_prisma_args(m.non_list_args),
        list_args: convert_list_args(m.list_args),
        nested_mutactions: convert_nested_mutactions(m.nested, Arc::clone(&project)),
    }
}

fn convert_nested_update_envelope(
    m: crate::protobuf::prisma::NestedUpdateNode,
    project: ProjectRef,
) -> NestedDatabaseMutaction {
    NestedDatabaseMutaction::UpdateNode(convert_nested_update(m, project))
}

fn convert_nested_update(m: crate::protobuf::prisma::NestedUpdateNode, project: ProjectRef) -> NestedUpdateNode {
    let relation_field = find_relation_field(Arc::clone(&project), m.model_name, m.field_name);
    NestedUpdateNode {
        relation_field: relation_field,
        where_: m.where_.map(|w| convert_node_select(w, Arc::clone(&project))),
        non_list_args: convert_prisma_args(m.non_list_args),
        list_args: convert_list_args(m.list_args),
        nested_mutactions: convert_nested_mutactions(m.nested, Arc::clone(&project)),
    }
}

fn convert_update_nodes(m: crate::protobuf::prisma::UpdateNodes, project: ProjectRef) -> TopLevelDatabaseMutaction {
    let model = project.schema().find_model(&m.model_name).unwrap();
    let update_nodes = UpdateNodes {
        model: Arc::clone(&model),
        filter: m.filter.into_filter(model),
        non_list_args: convert_prisma_args(m.non_list_args),
        list_args: convert_list_args(m.list_args),
    };
    TopLevelDatabaseMutaction::UpdateNodes(update_nodes)
}

fn convert_nested_update_nodes_envelope(
    m: crate::protobuf::prisma::NestedUpdateNodes,
    project: ProjectRef,
) -> NestedDatabaseMutaction {
    NestedDatabaseMutaction::UpdateNodes(convert_nested_update_nodes(m, project))
}

fn convert_nested_update_nodes(
    m: crate::protobuf::prisma::NestedUpdateNodes,
    project: ProjectRef,
) -> NestedUpdateNodes {
    let relation_field = find_relation_field(Arc::clone(&project), m.model_name, m.field_name);
    NestedUpdateNodes {
        relation_field: Arc::clone(&relation_field),
        filter: m.filter.map(|f| f.into_filter(relation_field.related_model())),
        non_list_args: convert_prisma_args(m.non_list_args),
        list_args: convert_list_args(m.list_args),
    }
}

fn convert_upsert(m: crate::protobuf::prisma::UpsertNode, project: ProjectRef) -> TopLevelDatabaseMutaction {
    let upsert_node = UpsertNode {
        where_: convert_node_select(m.where_, Arc::clone(&project)),
        create: convert_create(m.create, Arc::clone(&project)),
        update: convert_update(m.update, project),
    };
    TopLevelDatabaseMutaction::UpsertNode(upsert_node)
}

fn convert_nested_upsert_envelope(
    m: crate::protobuf::prisma::NestedUpsertNode,
    project: ProjectRef,
) -> NestedDatabaseMutaction {
    NestedDatabaseMutaction::UpsertNode(convert_nested_upsert(m, project))
}

fn convert_nested_upsert(m: crate::protobuf::prisma::NestedUpsertNode, project: ProjectRef) -> NestedUpsertNode {
    let relation_field = find_relation_field(Arc::clone(&project), m.model_name, m.field_name);
    NestedUpsertNode {
        relation_field: relation_field,
        where_: m.where_.map(|w| convert_node_select(w, Arc::clone(&project))),
        create: convert_nested_create(m.create, Arc::clone(&project)),
        update: convert_nested_update(m.update, Arc::clone(&project)),
    }
}

fn convert_delete(m: crate::protobuf::prisma::DeleteNode, project: ProjectRef) -> TopLevelDatabaseMutaction {
    let delete_node = DeleteNode {
        where_: convert_node_select(m.where_, project),
    };
    TopLevelDatabaseMutaction::DeleteNode(delete_node)
}

fn convert_nested_delete_envelope(
    m: crate::protobuf::prisma::NestedDeleteNode,
    project: ProjectRef,
) -> NestedDatabaseMutaction {
    NestedDatabaseMutaction::DeleteNode(convert_nested_delete(m, project))
}

fn convert_nested_delete(m: crate::protobuf::prisma::NestedDeleteNode, project: ProjectRef) -> NestedDeleteNode {
    NestedDeleteNode {
        relation_field: find_relation_field(Arc::clone(&project), m.model_name, m.field_name),
        where_: m.where_.map(|w| convert_node_select(w, project)),
    }
}

fn convert_delete_nodes(m: crate::protobuf::prisma::DeleteNodes, project: ProjectRef) -> TopLevelDatabaseMutaction {
    let model = project.schema().find_model(&m.model_name).unwrap();
    let delete_nodes = DeleteNodes {
        model: Arc::clone(&model),
        filter: m.filter.into_filter(model),
    };
    TopLevelDatabaseMutaction::DeleteNodes(delete_nodes)
}

fn convert_nested_delete_nodes_envelope(
    m: crate::protobuf::prisma::NestedDeleteNodes,
    project: ProjectRef,
) -> NestedDatabaseMutaction {
    NestedDatabaseMutaction::DeleteNodes(convert_nested_delete_nodes(m, project))
}

fn convert_nested_delete_nodes(
    m: crate::protobuf::prisma::NestedDeleteNodes,
    project: ProjectRef,
) -> NestedDeleteNodes {
    let relation_field = find_relation_field(project, m.model_name, m.field_name);
    NestedDeleteNodes {
        relation_field: Arc::clone(&relation_field),
        filter: m.filter.map(|f| f.into_filter(relation_field.related_model())),
    }
}

fn convert_reset(_: crate::protobuf::prisma::ResetData, project: ProjectRef) -> TopLevelDatabaseMutaction {
    let mutaction = ResetData { project };
    TopLevelDatabaseMutaction::ResetData(mutaction)
}

fn convert_nested_connect_envelope(
    m: crate::protobuf::prisma::NestedConnect,
    project: ProjectRef,
) -> NestedDatabaseMutaction {
    NestedDatabaseMutaction::Connect(convert_nested_connect(m, project))
}

fn convert_nested_connect(m: crate::protobuf::prisma::NestedConnect, project: ProjectRef) -> NestedConnect {
    let relation_field = project
        .schema()
        .find_model(&m.model_name)
        .unwrap()
        .fields()
        .find_from_relation_fields(&m.field_name)
        .unwrap();

    NestedConnect {
        relation_field: relation_field,
        where_: convert_node_select(m.where_, project),
        top_is_create: m.top_is_create,
    }
}

fn convert_nested_disconnect_envelope(
    m: crate::protobuf::prisma::NestedDisconnect,
    project: ProjectRef,
) -> NestedDatabaseMutaction {
    NestedDatabaseMutaction::Disconnect(convert_nested_disconnect(m, project))
}

fn convert_nested_disconnect(m: crate::protobuf::prisma::NestedDisconnect, project: ProjectRef) -> NestedDisconnect {
    let relation_field = project
        .schema()
        .find_model(&m.model_name)
        .unwrap()
        .fields()
        .find_from_relation_fields(&m.field_name)
        .unwrap();

    NestedDisconnect {
        relation_field: relation_field,
        where_: m.where_.map(|w| convert_node_select(w, project)),
    }
}

fn convert_nested_set_envelope(m: crate::protobuf::prisma::NestedSet, project: ProjectRef) -> NestedDatabaseMutaction {
    NestedDatabaseMutaction::Set(convert_nested_set(m, project))
}

fn convert_nested_set(m: crate::protobuf::prisma::NestedSet, project: ProjectRef) -> NestedSet {
    let relation_field = project
        .schema()
        .find_model(&m.model_name)
        .unwrap()
        .fields()
        .find_from_relation_fields(&m.field_name)
        .unwrap();

    NestedSet {
        relation_field: relation_field,
        wheres: m
            .wheres
            .into_iter()
            .map(|w| convert_node_select(w, Arc::clone(&project)))
            .collect(),
    }
}

fn convert_node_select(selector: crate::protobuf::prisma::NodeSelector, project: ProjectRef) -> NodeSelector {
    let model = project.schema().find_model(&selector.model_name).unwrap();
    let field = model.fields().find_from_scalar(&selector.field_name).unwrap();
    let value: PrismaValue = selector.value.into();
    NodeSelector { field, value }
}

fn convert_prisma_args(proto: crate::protobuf::prisma::PrismaArgs) -> PrismaArgs {
    let mut result = PrismaArgs::default();
    for arg in proto.args {
        result.insert(arg.key, arg.value);
    }
    result
}

fn convert_list_args(proto: crate::protobuf::prisma::PrismaArgs) -> Vec<(String, PrismaListValue)> {
    let mut result = vec![];
    for arg in proto.args {
        let value: PrismaListValue = arg.value.into();
        let tuple = (arg.key, value);
        result.push(tuple)
    }
    result
}

fn find_relation_field(project: ProjectRef, model: String, field: String) -> Arc<RelationField> {
    project
        .schema()
        .find_model(&model)
        .unwrap()
        .fields()
        .find_from_relation_fields(&field)
        .unwrap()
}

fn convert_mutaction_result(result: DatabaseMutactionResult) -> crate::protobuf::prisma::DatabaseMutactionResult {
    use crate::protobuf::prisma::database_mutaction_result;

    match result.typ {
        DatabaseMutactionResultType::Create => {
            let result = crate::protobuf::prisma::IdResult { id: result.id().into() };
            let typ = database_mutaction_result::Type::Create(result);

            crate::protobuf::prisma::DatabaseMutactionResult { type_: Some(typ) }
        }
        DatabaseMutactionResultType::Update => {
            let result = crate::protobuf::prisma::IdResult { id: result.id().into() };
            let typ = database_mutaction_result::Type::Update(result);

            crate::protobuf::prisma::DatabaseMutactionResult { type_: Some(typ) }
        }
        DatabaseMutactionResultType::Delete => {
            let result = crate::protobuf::prisma::IdResult { id: result.id().into() };
            let typ = database_mutaction_result::Type::Delete(result);

            crate::protobuf::prisma::DatabaseMutactionResult { type_: Some(typ) }
        }
        DatabaseMutactionResultType::Many => {
            let result = crate::protobuf::prisma::ManyNodesResult {
                count: result.count() as u32,
            };
            let typ = database_mutaction_result::Type::Many(result);
            crate::protobuf::prisma::DatabaseMutactionResult { type_: Some(typ) }
        }
        DatabaseMutactionResultType::Unit => {
            let result = crate::protobuf::prisma::Unit {};
            let typ = database_mutaction_result::Type::Unit(result);
            crate::protobuf::prisma::DatabaseMutactionResult { type_: Some(typ) }
        }

        // x => panic!("can't handle result type {:?}", x),
    }
}
