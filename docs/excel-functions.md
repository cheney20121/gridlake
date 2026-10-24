# Excel 函数兼容清单

> 由 `web/scripts/formula-catalog.mjs` 从表格内核（Univer `@univerjs/engine-formula` 1.0.2）的函数目录生成，不要手改；CI 会检查它是最新的（`pnpm formula:catalog --check`）。

**按表格内核整理的 Excel 函数目录共 536 个（含少数 Google 表格的同名扩展），支持 509 个（95%）**，包括 `XLOOKUP`、`FILTER`、`UNIQUE`、`SORTBY`、`LET`、`LAMBDA`、`MAP`、`VSTACK`、`TEXTSPLIT`、`REGEXTEST`、`GROUPBY` 这些较新的动态数组函数。另有 16 个 Gridlake 平台函数（Excel 里没有）。

- 公式在浏览器里计算；引用大数据集的计算请用平台函数（`DSUMIFS`、`SQL` 等），在服务端算，见 [formulas.md](formulas.md)。
- 导入 xlsx：文件里的 `_xlfn.` / `_xlws.` / `_xlpm.` 前缀自动去掉；用到不支持的函数时，导入结果里会点名是哪几个函数（这些公式显示 `#NAME?`）。
- 导出 xlsx：Excel 2010 之后新增的 160 个函数自动加 `_xlfn.` 前缀（`FILTER`、`SORT` 为 `_xlfn._xlws.`），旧版 Excel 打开时能识别；平台函数写成计算结果，原公式记在批注里。

## 不支持的函数（27）

| 函数 | 类别 | 原因 / 替代 |
|---|---|---|
| `ISO.CEILING` | 数学与三角 | 表格内核尚未实现 |
| `FORECAST.ETS` | 统计 | 表格内核尚未实现 |
| `FORECAST.ETS.CONFINT` | 统计 | 表格内核尚未实现 |
| `FORECAST.ETS.SEASONALITY` | 统计 | 表格内核尚未实现 |
| `FORECAST.ETS.STAT` | 统计 | 表格内核尚未实现 |
| `GETPIVOTDATA` | 查找与引用 | Gridlake 的透视表是服务端计算后写成的普通工作表，直接引用单元格，或用 DSUMIFS 按条件汇总 |
| `PIVOTBY` | 查找与引用 | 内核尚未实现；可用「透视分析」或 =SQL("… GROUP BY …") |
| `RTD` | 查找与引用 | 需要 Windows 上的实时数据服务器（COM） |
| `TRIMRANGE` | 查找与引用 | 表格内核尚未实现 |
| `CALL` | 文本 | 调用本机程序库，网页里不能运行 |
| `COPILOT` | 文本 | 需要微软的 AI 服务；可用右上角的 AI 助手 |
| `DETECTLANGUAGE` | 文本 | 需要微软的翻译服务 |
| `EUROCONVERT` | 文本 | 依赖 Excel 的欧元转换加载项 |
| `PHONETIC` | 文本 | 读取日文注音（ふりがな）信息，表格里不保存 |
| `REGISTER.ID` | 文本 | 调用本机程序库，网页里不能运行 |
| `TRANSLATE` | 文本 | 需要微软的翻译服务 |
| `INFO` | 信息 | 返回操作系统、内存等本机环境信息 |
| `ISOMITTED` | 信息 | 表格内核尚未实现 |
| `AMORDEGRC` | 财务 | 表格内核尚未实现 |
| `CUBEKPIMEMBER` | 多维数据集 | 需要连接 OLAP / Power Pivot 数据模型；大数据请用数据集与透视表 |
| `CUBEMEMBER` | 多维数据集 | 需要连接 OLAP / Power Pivot 数据模型；大数据请用数据集与透视表 |
| `CUBEMEMBERPROPERTY` | 多维数据集 | 需要连接 OLAP / Power Pivot 数据模型；大数据请用数据集与透视表 |
| `CUBERANKEDMEMBER` | 多维数据集 | 需要连接 OLAP / Power Pivot 数据模型；大数据请用数据集与透视表 |
| `CUBESET` | 多维数据集 | 需要连接 OLAP / Power Pivot 数据模型；大数据请用数据集与透视表 |
| `CUBESETCOUNT` | 多维数据集 | 需要连接 OLAP / Power Pivot 数据模型；大数据请用数据集与透视表 |
| `CUBEVALUE` | 多维数据集 | 需要连接 OLAP / Power Pivot 数据模型；大数据请用数据集与透视表 |
| `WEBSERVICE` | Web | 浏览器里不能任意访问外部网址；跨工作簿取数用 IMPORTRANGE，接口数据用「数据连接」 |

## 平台函数（16）

| 函数 | 说明 |
|---|---|
| `DSUMIFS` | 对数据集（服务端大数据表）按条件求和 |
| `DCOUNTIFS` | 对数据集按条件计数 |
| `DAVERAGEIFS` | 对数据集按条件求平均 |
| `DMAXIFS` | 对数据集按条件取最大值 |
| `DMINIFS` | 对数据集按条件取最小值 |
| `DCOUNTDISTINCTIFS` | 对数据集按条件统计不重复的取值个数 |
| `DLOOKUP` | 在数据集中查找，返回第一条匹配行的值 |
| `TSUM` | 对工作表里的数据表求和（按它当前的筛选） |
| `TCOUNT` | 数据表的行数 / 按条件计数 |
| `TAVERAGE` | 数据表某列的平均值 |
| `TMIN` | 数据表某列的最小值 |
| `TMAX` | 数据表某列的最大值 |
| `TVALUE` | 数据表当前排序下第 N 行的值 |
| `SQL` | 在服务端执行只读 SQL，结果带表头铺开 |
| `SQLVALUE` | 执行只读 SQL，取第一行第一列 |
| `IMPORTRANGE` | 读取另一个工作簿的区域 |

## 按类别

标 <sup>新</sup> 的是 Excel 2010 之后新增的函数（导出时加前缀）。

### 数学与三角（支持 82 / 83）

`ABS` `ACOS` `ACOSH` `ACOT`<sup>新</sup> `ACOTH`<sup>新</sup> `AGGREGATE`<sup>新</sup> `ARABIC`<sup>新</sup> `ASIN` `ASINH` `ATAN` `ATAN2` `ATANH` `BASE`<sup>新</sup> `CEILING` `CEILING.MATH`<sup>新</sup> `CEILING.PRECISE`<sup>新</sup> `COMBIN` `COMBINA`<sup>新</sup> `COS` `COSH` `COT`<sup>新</sup> `COTH`<sup>新</sup> `CSC`<sup>新</sup> `CSCH`<sup>新</sup> `DECIMAL`<sup>新</sup> `DEGREES` `EVEN` `EXP` `FACT` `FACTDOUBLE` `FLOOR` `FLOOR.MATH`<sup>新</sup> `FLOOR.PRECISE`<sup>新</sup> `GCD` `INT` `LCM` `LET`<sup>新</sup> `LN` `LOG` `LOG10` `MDETERM` `MINVERSE` `MMULT` `MOD` `MROUND` `MULTINOMIAL` `MUNIT`<sup>新</sup> `ODD` `PI` `POWER` `PRODUCT` `QUOTIENT` `RADIANS` `RAND` `RANDARRAY`<sup>新</sup> `RANDBETWEEN` `ROMAN` `ROUND` `ROUNDBANK` `ROUNDDOWN` `ROUNDUP` `SEC`<sup>新</sup> `SECH`<sup>新</sup> `SEQUENCE`<sup>新</sup> `SERIESSUM` `SIGN` `SIN` `SINH` `SQRT` `SQRTPI` `SUBTOTAL` `SUM` `SUMIF` `SUMIFS` `SUMPRODUCT` `SUMSQ` `SUMX2MY2` `SUMX2PY2` `SUMXMY2` `TAN` `TANH` `TRUNC`

不支持：`ISO.CEILING`

### 统计（支持 109 / 113）

`AVEDEV` `AVERAGE` `AVERAGE.WEIGHTED` `AVERAGEA` `AVERAGEIF` `AVERAGEIFS` `BETA.DIST`<sup>新</sup> `BETA.INV`<sup>新</sup> `BINOM.DIST`<sup>新</sup> `BINOM.DIST.RANGE`<sup>新</sup> `BINOM.INV`<sup>新</sup> `CHISQ.DIST`<sup>新</sup> `CHISQ.DIST.RT`<sup>新</sup> `CHISQ.INV`<sup>新</sup> `CHISQ.INV.RT`<sup>新</sup> `CHISQ.TEST`<sup>新</sup> `CONFIDENCE.NORM`<sup>新</sup> `CONFIDENCE.T`<sup>新</sup> `CORREL` `COUNT` `COUNTA` `COUNTBLANK` `COUNTIF` `COUNTIFS` `COVARIANCE.P`<sup>新</sup> `COVARIANCE.S`<sup>新</sup> `DEVSQ` `EXPON.DIST`<sup>新</sup> `F.DIST`<sup>新</sup> `F.DIST.RT`<sup>新</sup> `F.INV`<sup>新</sup> `F.INV.RT`<sup>新</sup> `F.TEST`<sup>新</sup> `FISHER` `FISHERINV` `FORECAST` `FORECAST.LINEAR`<sup>新</sup> `FREQUENCY` `GAMMA`<sup>新</sup> `GAMMA.DIST`<sup>新</sup> `GAMMA.INV`<sup>新</sup> `GAMMALN` `GAMMALN.PRECISE`<sup>新</sup> `GAUSS`<sup>新</sup> `GEOMEAN` `GROWTH` `HARMEAN` `HYPGEOM.DIST`<sup>新</sup> `INTERCEPT` `KURT` `LARGE` `LINEST` `LOGEST` `LOGNORM.DIST`<sup>新</sup> `LOGNORM.INV`<sup>新</sup> `MARGINOFERROR` `MAX` `MAXA` `MAXIFS`<sup>新</sup> `MEDIAN` `MIN` `MINA` `MINIFS`<sup>新</sup> `MODE.MULT`<sup>新</sup> `MODE.SNGL`<sup>新</sup> `NEGBINOM.DIST`<sup>新</sup> `NORM.DIST`<sup>新</sup> `NORM.INV`<sup>新</sup> `NORM.S.DIST`<sup>新</sup> `NORM.S.INV`<sup>新</sup> `PEARSON` `PERCENTILE.EXC`<sup>新</sup> `PERCENTILE.INC`<sup>新</sup> `PERCENTRANK.EXC`<sup>新</sup> `PERCENTRANK.INC`<sup>新</sup> `PERMUT` `PERMUTATIONA`<sup>新</sup> `PHI`<sup>新</sup> `POISSON.DIST`<sup>新</sup> `PROB` `QUARTILE.EXC`<sup>新</sup> `QUARTILE.INC`<sup>新</sup> `RANK.AVG`<sup>新</sup> `RANK.EQ`<sup>新</sup> `RSQ` `SKEW` `SKEW.P`<sup>新</sup> `SLOPE` `SMALL` `STANDARDIZE` `STDEV.P`<sup>新</sup> `STDEV.S`<sup>新</sup> `STDEVA` `STDEVPA` `STEYX` `T.DIST`<sup>新</sup> `T.DIST.2T`<sup>新</sup> `T.DIST.RT`<sup>新</sup> `T.INV`<sup>新</sup> `T.INV.2T`<sup>新</sup> `T.TEST`<sup>新</sup> `TREND` `TRIMMEAN` `VAR.P`<sup>新</sup> `VAR.S`<sup>新</sup> `VARA` `VARPA` `WEIBULL.DIST`<sup>新</sup> `Z.TEST`<sup>新</sup>

不支持：`FORECAST.ETS` `FORECAST.ETS.CONFINT` `FORECAST.ETS.SEASONALITY` `FORECAST.ETS.STAT`

### 查找与引用（支持 36 / 40）

`ADDRESS` `ANCHORARRAY` `AREAS` `CHOOSE` `CHOOSECOLS`<sup>新</sup> `CHOOSEROWS`<sup>新</sup> `COLUMN` `COLUMNS` `DROP`<sup>新</sup> `EXPAND`<sup>新</sup> `FILTER`<sup>新</sup> `FORMULATEXT`<sup>新</sup> `HLOOKUP` `HSTACK`<sup>新</sup> `HYPERLINK` `IMAGE`<sup>新</sup> `INDEX` `INDIRECT` `LOOKUP` `MATCH` `OFFSET` `ROW` `ROWS` `SORT`<sup>新</sup> `SORTBY`<sup>新</sup> `TAKE`<sup>新</sup> `TOCOL`<sup>新</sup> `TOROW`<sup>新</sup> `TRANSPOSE` `UNIQUE`<sup>新</sup> `VLOOKUP` `VSTACK`<sup>新</sup> `WRAPCOLS`<sup>新</sup> `WRAPROWS`<sup>新</sup> `XLOOKUP`<sup>新</sup> `XMATCH`<sup>新</sup>

不支持：`GETPIVOTDATA` `PIVOTBY` `RTD` `TRIMRANGE`

### 文本（支持 48 / 55）

`ARRAYTOTEXT`<sup>新</sup> `ASC` `BAHTTEXT` `CHAR` `CLEAN` `CODE` `CONCAT`<sup>新</sup> `CONCATENATE` `DBCS` `DOLLAR` `EXACT` `FIND` `FINDB` `FIXED` `LEFT` `LEFTB` `LEN` `LENB` `LOWER` `MID` `MIDB` `NUMBERSTRING` `NUMBERVALUE`<sup>新</sup> `PROPER` `REGEXEXTRACT`<sup>新</sup> `REGEXMATCH` `REGEXREPLACE`<sup>新</sup> `REGEXTEST`<sup>新</sup> `REPLACE` `REPLACEB` `REPT` `RIGHT` `RIGHTB` `SEARCH` `SEARCHB` `SUBSTITUTE` `T` `TEXT` `TEXTAFTER`<sup>新</sup> `TEXTBEFORE`<sup>新</sup> `TEXTJOIN`<sup>新</sup> `TEXTSPLIT`<sup>新</sup> `TRIM` `UNICHAR`<sup>新</sup> `UNICODE`<sup>新</sup> `UPPER` `VALUE` `VALUETOTEXT`<sup>新</sup>

不支持：`CALL` `COPILOT` `DETECTLANGUAGE` `EUROCONVERT` `PHONETIC` `REGISTER.ID` `TRANSLATE`

### 逻辑（支持 21 / 21）

`AND` `BYCOL`<sup>新</sup> `BYROW`<sup>新</sup> `FALSE` `GROUPBY`<sup>新</sup> `IF` `IFERROR` `IFNA`<sup>新</sup> `IFS`<sup>新</sup> `LAMBDA`<sup>新</sup> `LET`<sup>新</sup> `MAKEARRAY`<sup>新</sup> `MAP`<sup>新</sup> `NOT` `OR` `PERCENTOF`<sup>新</sup> `REDUCE`<sup>新</sup> `SCAN`<sup>新</sup> `SWITCH`<sup>新</sup> `TRUE` `XOR`<sup>新</sup>

### 日期与时间（支持 27 / 27）

`DATE` `DATEDIF` `DATEVALUE` `DAY` `DAYS`<sup>新</sup> `DAYS360` `EDATE` `EOMONTH` `EPOCHTODATE` `HOUR` `ISOWEEKNUM`<sup>新</sup> `MINUTE` `MONTH` `NETWORKDAYS` `NETWORKDAYS.INTL`<sup>新</sup> `NOW` `SECOND` `TIME` `TIMEVALUE` `TODAY` `TO_DATE` `WEEKDAY` `WEEKNUM` `WORKDAY` `WORKDAY.INTL`<sup>新</sup> `YEAR` `YEARFRAC`

### 信息（支持 23 / 25）

`CELL` `ERROR.TYPE` `ISBETWEEN` `ISBLANK` `ISDATE` `ISEMAIL` `ISERR` `ISERROR` `ISEVEN` `ISFORMULA`<sup>新</sup> `ISLOGICAL` `ISNA` `ISNONTEXT` `ISNUMBER` `ISODD` `ISREF` `ISTEXT` `ISURL` `N` `NA` `SHEET`<sup>新</sup> `SHEETS`<sup>新</sup> `TYPE`

不支持：`INFO` `ISOMITTED`

### 财务（支持 54 / 55）

`ACCRINT` `ACCRINTM` `AMORLINC` `COUPDAYBS` `COUPDAYS` `COUPDAYSNC` `COUPNCD` `COUPNUM` `COUPPCD` `CUMIPMT` `CUMPRINC` `DB` `DDB` `DISC` `DOLLARDE` `DOLLARFR` `DURATION` `EFFECT` `FV` `FVSCHEDULE` `INTRATE` `IPMT` `IRR` `ISPMT` `MDURATION` `MIRR` `NOMINAL` `NPER` `NPV` `ODDFPRICE` `ODDFYIELD` `ODDLPRICE` `ODDLYIELD` `PDURATION`<sup>新</sup> `PMT` `PPMT` `PRICE` `PRICEDISC` `PRICEMAT` `PV` `RATE` `RECEIVED` `RRI`<sup>新</sup> `SLN` `SYD` `TBILLEQ` `TBILLPRICE` `TBILLYIELD` `VDB` `XIRR` `XNPV` `YIELD` `YIELDDISC` `YIELDMAT`

不支持：`AMORDEGRC`

### 工程（支持 57 / 57）

`BESSELI` `BESSELJ` `BESSELK` `BESSELY` `BIN2DEC` `BIN2HEX` `BIN2OCT` `BITAND`<sup>新</sup> `BITLSHIFT`<sup>新</sup> `BITOR`<sup>新</sup> `BITRSHIFT`<sup>新</sup> `BITXOR`<sup>新</sup> `COMPLEX` `CONVERT` `DEC2BIN` `DEC2HEX` `DEC2OCT` `DELTA` `ERF` `ERF.PRECISE`<sup>新</sup> `ERFC` `ERFC.PRECISE`<sup>新</sup> `GESTEP` `HEX2BIN` `HEX2DEC` `HEX2OCT` `IMABS` `IMAGINARY` `IMARGUMENT` `IMCONJUGATE` `IMCOS` `IMCOSH`<sup>新</sup> `IMCOT`<sup>新</sup> `IMCOTH` `IMCSC`<sup>新</sup> `IMCSCH`<sup>新</sup> `IMDIV` `IMEXP` `IMLN` `IMLOG` `IMLOG10` `IMLOG2` `IMPOWER` `IMPRODUCT` `IMREAL` `IMSEC`<sup>新</sup> `IMSECH`<sup>新</sup> `IMSIN` `IMSINH`<sup>新</sup> `IMSQRT` `IMSUB` `IMSUM` `IMTAN`<sup>新</sup> `IMTANH` `OCT2BIN` `OCT2DEC` `OCT2HEX`

### 数据库（支持 12 / 12）

`DAVERAGE` `DCOUNT` `DCOUNTA` `DGET` `DMAX` `DMIN` `DPRODUCT` `DSTDEV` `DSTDEVP` `DSUM` `DVAR` `DVARP`

### 兼容性（旧版函数）（支持 38 / 38）

`BETADIST` `BETAINV` `BINOMDIST` `CHIDIST` `CHIINV` `CHITEST` `CONFIDENCE` `COVAR` `CRITBINOM` `EXPONDIST` `FDIST` `FINV` `FTEST` `GAMMADIST` `GAMMAINV` `HYPGEOMDIST` `LOGINV` `LOGNORMDIST` `MODE` `NEGBINOMDIST` `NORMDIST` `NORMINV` `NORMSDIST` `NORMSINV` `PERCENTILE` `PERCENTRANK` `POISSON` `QUARTILE` `RANK` `STDEV` `STDEVP` `TDIST` `TINV` `TTEST` `VAR` `VARP` `WEIBULL` `ZTEST`

### 多维数据集（支持 0 / 7）

不支持：`CUBEKPIMEMBER` `CUBEMEMBER` `CUBEMEMBERPROPERTY` `CUBERANKEDMEMBER` `CUBESET` `CUBESETCOUNT` `CUBEVALUE`

### Web（支持 2 / 3）

`ENCODEURL`<sup>新</sup> `FILTERXML`<sup>新</sup>

不支持：`WEBSERVICE`

### Google 表格函数（额外支持）

Excel 里没有、Google 表格里有的函数，这里也能用（导出 xlsx 后 Excel 不认识）：`ARRAY_CONSTRAIN` `FLATTEN`
