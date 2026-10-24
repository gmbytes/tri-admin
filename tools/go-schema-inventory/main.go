// Command go-schema-inventory materializes the source models only in the isolated development database.
package main

import (
	"encoding/json"
	"flag"
	"fmt"
	"net/url"
	"os"
	"path/filepath"
	"strings"

	adapter "github.com/casbin/gorm-adapter/v3"
	"github.com/flipped-aurora/gin-vue-admin/server/global"
	"github.com/flipped-aurora/gin-vue-admin/server/initialize"
	driver "github.com/go-sql-driver/mysql"
	"go.uber.org/zap"
	"gorm.io/driver/mysql"
	"gorm.io/gorm"
	"gorm.io/gorm/logger"
)

type column struct {
	Table    string  `json:"table" gorm:"column:table_name"`
	Name     string  `json:"name" gorm:"column:column_name"`
	Type     string  `json:"type" gorm:"column:column_type"`
	Nullable string  `json:"nullable" gorm:"column:is_nullable"`
	Default  *string `json:"default" gorm:"column:column_default"`
	Extra    string  `json:"extra" gorm:"column:extra"`
}

type index struct {
	Table     string `json:"table" gorm:"column:table_name"`
	Name      string `json:"name" gorm:"column:index_name"`
	Column    string `json:"column" gorm:"column:column_name"`
	Sequence  int    `json:"sequence" gorm:"column:seq_in_index"`
	NonUnique int    `json:"non_unique" gorm:"column:non_unique"`
}

func main() {
	envFile := flag.String("env-file", "", "tri-admin development env file")
	output := flag.String("output", "", "schema JSON output path")
	sqlOutput := flag.String("sql-output", "", "baseline migration output path")
	flag.Parse()
	if err := run(*envFile, *output, *sqlOutput); err != nil {
		fmt.Fprintln(os.Stderr, err)
		os.Exit(1)
	}
}

func run(envFile, output, sqlOutput string) error {
	envContent, err := os.ReadFile(envFile)
	if err != nil { return fmt.Errorf("read development env: %w", err) }
	values := map[string]string{}
	for _, line := range strings.Split(string(envContent), "\n") {
		key, value, ok := strings.Cut(strings.TrimSpace(line), "=")
		if ok { values[key] = value }
	}
	databaseURL, err := url.Parse(values["TRI_LEGACY_DATABASE_URL"])
	if err != nil { return fmt.Errorf("invalid development database URL") }
	if databaseURL.Scheme != "mysql" || databaseURL.Hostname() != "127.0.0.1" || databaseURL.Port() != "13316" || databaseURL.Path != "/tri_admin_legacy" {
		return fmt.Errorf("schema capture is restricted to 127.0.0.1:13316/tri_admin_legacy")
	}
	password, _ := databaseURL.User.Password()
	conf := driver.NewConfig()
	conf.User, conf.Passwd = databaseURL.User.Username(), password
	conf.Net, conf.Addr, conf.DBName, conf.ParseTime = "tcp", databaseURL.Host, "tri_admin_legacy", true
	db, err := gorm.Open(mysql.New(mysql.Config{DSN: conf.FormatDSN(),DefaultStringSize:191}), &gorm.Config{DisableForeignKeyConstraintWhenMigrating:true,Logger:logger.Default.LogMode(logger.Silent)})
	if err != nil { return fmt.Errorf("connect isolated schema database failed") }
	sqlDB, err := db.DB()
	if err != nil { return err }
	defer sqlDB.Close()
	global.GVA_LOG = zap.NewNop()
	global.GVA_DB = db
	initialize.RegisterTables()
	if err := db.AutoMigrate(&adapter.CasbinRule{}); err != nil { return err }
	var columns []column
	if err := db.Raw("SELECT TABLE_NAME AS table_name,COLUMN_NAME AS column_name,COLUMN_TYPE AS column_type,IS_NULLABLE AS is_nullable,COLUMN_DEFAULT AS column_default,EXTRA AS extra FROM information_schema.columns WHERE table_schema=DATABASE() ORDER BY table_name,ordinal_position").Scan(&columns).Error; err != nil { return err }
	var indexes []index
	if err := db.Raw("SELECT TABLE_NAME AS table_name,INDEX_NAME AS index_name,COLUMN_NAME AS column_name,SEQ_IN_INDEX AS seq_in_index,NON_UNIQUE AS non_unique FROM information_schema.statistics WHERE table_schema=DATABASE() ORDER BY table_name,index_name,seq_in_index").Scan(&indexes).Error; err != nil { return err }
	var tables []string
	if err := db.Raw("SELECT table_name FROM information_schema.tables WHERE table_schema=DATABASE() ORDER BY table_name").Scan(&tables).Error; err != nil { return err }
	if len(tables) < 30 { return fmt.Errorf("incomplete source schema capture: %d tables",len(tables)) }
	for _, entry := range columns { if entry.Table == "" || entry.Name == "" || entry.Type == "" { return fmt.Errorf("schema column metadata aliases were not populated") } }
	data, err := json.MarshalIndent(struct {Columns []column `json:"columns"`; Indexes []index `json:"indexes"`}{columns,indexes},"","  ")
	if err != nil { return err }
	if err := os.MkdirAll(filepath.Dir(output),0755); err != nil { return err }
	if err := os.WriteFile(output,append(data,'\n'),0644); err != nil { return err }
	var statements []string
	statements = append(statements,"-- Generated from vue-admin Gorm models against isolated MySQL 8.4; no data or credentials.\n")
	for _, table := range tables {
		var name, definition string
		if err := sqlDB.QueryRow("SHOW CREATE TABLE `"+strings.ReplaceAll(table,"`","``")+"`").Scan(&name,&definition); err != nil { return err }
		definition = strings.Replace(definition,"CREATE TABLE ","CREATE TABLE IF NOT EXISTS ",1)
		statements = append(statements,definition+";\n")
	}
	if err := os.MkdirAll(filepath.Dir(sqlOutput),0755); err != nil { return err }
	if err := os.WriteFile(sqlOutput,[]byte(strings.Join(statements,"\n")),0644); err != nil { return err }
	fmt.Printf("Captured %d tables, %d columns and %d index fields from isolated schema.\n",len(tables),len(columns),len(indexes))
	return nil
}
